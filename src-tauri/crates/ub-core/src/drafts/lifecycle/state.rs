//! The lifecycle's queries and transitions. Pure, on the draft value: the
//! caller writes it back with `tx.drafts().put`. A transition that may
//! change nothing says whether it did, so the caller writes only then.

use ub_domain::{GmailDraftId, GmailMessageId, GmailThreadId};
use ub_store::{Draft, DraftAttachment, DraftStage, DraftWait};

use crate::drafts::text::is_blank;

/// Gmail's ids for a draft, as a write of it returns them or a read finds
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GmailIds {
    /// The Gmail draft.
    pub(crate) draft: GmailDraftId,
    /// Its message.
    pub(crate) message: GmailMessageId,
    /// Its message's thread.
    pub(crate) thread: GmailThreadId,
}

impl GmailIds {
    /// The ids of a Gmail draft as Gmail answered a write of it.
    pub(crate) fn of(written: &ub_gmail::Draft) -> Self {
        Self {
            draft: written.id.clone(),
            message: written.message.id.clone(),
            thread: written.message.thread_id.clone(),
        }
    }
}

/// How the next write of a draft reaches Gmail (SPEC Gmail errors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GmailWrite<'d> {
    /// `drafts.update` of its Gmail draft; a create if Gmail no longer has
    /// it.
    Update(&'d GmailDraftId),
    /// A `drafts.create` went out with no answer stored: Gmail may have the
    /// draft, so it is looked for by its `X-Ubermensch-Draft` header first.
    Recover,
    /// `drafts.create`.
    Create,
}

// The stage: the untouched rule.

/// A fresh composer's first save, its content filled by the addressing
/// rules: closing that composer untouched discards it. Its defaults are that
/// content with a blank body.
pub(crate) fn start_fresh(draft: &mut Draft) {
    let defaults = draft.content().with_body(String::new());
    draft.lifecycle.stage = DraftStage::Fresh { defaults };
}

/// Whether its first composer is still open.
pub(crate) fn is_fresh(draft: &Draft) -> bool {
    matches!(draft.lifecycle.stage, DraftStage::Fresh { .. })
}

/// Whether the draft is as its composer started it: the defaults' From,
/// recipients, subject, and attachments, and the same body or a blank one.
/// A kept draft is never untouched.
pub(crate) fn is_untouched(draft: &Draft) -> bool {
    match &draft.lifecycle.stage {
        DraftStage::Fresh { defaults } | DraftStage::Opened { defaults } => {
            defaults.is_untouched_by(draft.content(), is_blank(&draft.body_html))
        }
        DraftStage::Kept => false,
    }
}

/// Whether closing its composer discards it: fresh and untouched.
pub(crate) fn discards_on_close(draft: &Draft) -> bool {
    is_fresh(draft) && is_untouched(draft)
}

/// Its first composer closed, or sent it: it is fresh no more. Returns
/// whether it was.
pub(crate) fn composer_closed(draft: &mut Draft) -> bool {
    let DraftStage::Fresh { defaults } = &mut draft.lifecycle.stage else {
        return false;
    };
    let defaults = std::mem::take(defaults);
    draft.lifecycle.stage = DraftStage::Opened { defaults };
    true
}

/// The user meant to keep it: its send came back, was refused, or was
/// undone while waiting. It is never untouched again, so never discarded as
/// untouched.
pub(crate) fn keep(draft: &mut Draft) {
    draft.lifecycle.stage = DraftStage::Kept;
}

// Waits before a write to Gmail.

/// It quotes its target's snippet, since the target's body was not stored:
/// it waits for that body before it is written to Gmail.
pub(crate) fn await_target_body(draft: &mut Draft) {
    draft.lifecycle.waits_for = Some(DraftWait::TargetBody);
}

/// Whether it waits for its target's body.
pub(crate) fn awaits_target_body(draft: &Draft) -> bool {
    draft.lifecycle.waits_for == Some(DraftWait::TargetBody)
}

/// Its target's body landed and its quote is whole now: the wait ends, and
/// the attachments that came with the body (a forward's) count as defaults,
/// so an untouched forward stays untouched.
pub(crate) fn body_landed(draft: &mut Draft, added: &[DraftAttachment]) {
    target_body_gone(draft);
    if let DraftStage::Fresh { defaults } | DraftStage::Opened { defaults } =
        &mut draft.lifecycle.stage
    {
        defaults
            .attachments
            .extend(added.iter().map(|a| a.id.clone()));
    }
}

/// A write can't wait for its target's body any longer, which is not stored
/// (Gmail no longer has the target): the snippet's quote stays. Returns
/// whether it waited.
pub(crate) fn target_body_gone(draft: &mut Draft) -> bool {
    let waited = awaits_target_body(draft);
    if waited {
        draft.lifecycle.waits_for = None;
    }
    waited
}

/// Whether it waits for its parts to download, read from Gmail.
pub(crate) fn awaits_parts(draft: &Draft) -> bool {
    draft.lifecycle.waits_for == Some(DraftWait::Parts)
}

/// Its parts are downloaded. Returns whether it waited for them.
pub(crate) fn parts_downloaded(draft: &mut Draft) -> bool {
    let waited = awaits_parts(draft);
    if waited {
        draft.lifecycle.waits_for = None;
    }
    waited
}

// Gmail's copy.

/// Its Gmail draft, from the last write.
pub(crate) fn gmail_draft(draft: &Draft) -> Option<&GmailDraftId> {
    draft.lifecycle.gmail_draft_id.as_ref()
}

/// Its Gmail draft's message, from the last write.
pub(crate) fn gmail_message(draft: &Draft) -> Option<&GmailMessageId> {
    draft.lifecycle.gmail_message_id.as_ref()
}

/// Its Gmail draft's thread, from the last write. Kept when Gmail's draft
/// goes, so the next one stays on that thread.
pub(crate) fn gmail_thread(draft: &Draft) -> Option<&GmailThreadId> {
    draft.lifecycle.gmail_thread_id.as_ref()
}

/// How its next write reaches Gmail.
pub(crate) fn gmail_write(draft: &Draft) -> GmailWrite<'_> {
    match gmail_draft(draft) {
        Some(id) => GmailWrite::Update(id),
        None if draft.lifecycle.create_sent => GmailWrite::Recover,
        None => GmailWrite::Create,
    }
}

/// A `drafts.create` is about to go out for it, recorded first so an
/// answer lost to a quit is looked for on the next write. Returns whether
/// that changed it.
pub(crate) fn begin_create(draft: &mut Draft) -> bool {
    let changed = !draft.lifecycle.create_sent;
    draft.lifecycle.create_sent = true;
    changed
}

/// Gmail has it as `ids`: a write answered, or a Gmail draft with its
/// header was adopted. A create in question is settled.
pub(crate) fn written(draft: &mut Draft, ids: GmailIds) {
    let life = &mut draft.lifecycle;
    life.gmail_draft_id = Some(ids.draft);
    life.gmail_message_id = Some(ids.message);
    life.gmail_thread_id = Some(ids.thread);
    life.create_sent = false;
}

/// Gmail's draft went (sent or deleted elsewhere) while a composer has it
/// open: it keeps its text, and its next write creates a Gmail draft on the
/// same thread, or in a thread of its own if that thread went with it.
pub(crate) fn gmail_gone(draft: &mut Draft) {
    draft.lifecycle.gmail_draft_id = None;
    draft.lifecycle.gmail_message_id = None;
}

/// Gmail changed it while a composer has it open: the change is read when
/// the composer closes. Returns whether that changed it.
pub(crate) fn gmail_changed(draft: &mut Draft) -> bool {
    let changed = !draft.lifecycle.gmail_change_skipped;
    draft.lifecycle.gmail_change_skipped = true;
    changed
}

/// Whether a Gmail change was skipped while a composer had it open.
pub(crate) fn has_skipped_gmail_change(draft: &Draft) -> bool {
    draft.lifecycle.gmail_change_skipped
}

/// An edit was saved: it wins over a Gmail change skipped meanwhile, which
/// is not read on close (last writer wins).
pub(crate) fn edited(draft: &mut Draft) {
    draft.lifecycle.gmail_change_skipped = false;
}

/// Its content was read from Gmail as `ids`, and the caller replaced it:
/// it waits for its parts when it has attachments (`drafts.update`
/// replaces the whole message, so the next write must carry them), and no
/// Gmail change is left to read.
pub(crate) fn read_from_gmail(draft: &mut Draft, ids: GmailIds, has_parts: bool) {
    draft.lifecycle.waits_for = has_parts.then_some(DraftWait::Parts);
    written(draft, ids);
    draft.lifecycle.gmail_change_skipped = false;
}

#[cfg(test)]
mod tests {
    use ub_domain::{AccountId, Addr, DraftId, DraftKind, Millis};
    use ub_store::DraftLifecycle;

    use super::*;

    fn draft() -> Draft {
        let mut d = Draft::new(
            DraftId::from("d"),
            AccountId(1),
            DraftKind::Reply,
            Millis(1),
        );
        d.to = vec![Addr::new("bob@example.org")];
        d.subject = "Re: Lunch".to_owned();
        d
    }

    fn fresh() -> Draft {
        let mut d = draft();
        start_fresh(&mut d);
        d
    }

    fn opened() -> Draft {
        let mut d = fresh();
        composer_closed(&mut d);
        d
    }

    fn kept() -> Draft {
        draft()
    }

    fn ids(n: &str) -> GmailIds {
        GmailIds {
            draft: GmailDraftId::from(format!("r{n}")),
            message: GmailMessageId::from(format!("m{n}")),
            thread: GmailThreadId::from(format!("t{n}")),
        }
    }

    fn attachment(id: &str) -> DraftAttachment {
        DraftAttachment {
            id: id.to_owned(),
            filename: format!("{id}.pdf"),
            mime_type: "application/pdf".to_owned(),
            size: 1,
            path: None,
            source: None,
            added: None,
        }
    }

    #[test]
    fn a_new_draft_is_kept_with_no_wait_and_no_gmail_copy() {
        let d = draft();
        assert_eq!(d.lifecycle, DraftLifecycle::default());
        assert!(!is_fresh(&d) && !is_untouched(&d) && !discards_on_close(&d));
        assert_eq!(gmail_write(&d), GmailWrite::Create);
    }

    #[test]
    fn a_fresh_draft_is_untouched_with_its_defaults_or_a_blank_body() {
        let mut d = fresh();
        assert!(is_fresh(&d));
        assert!(discards_on_close(&d));
        d.body_html = "<p><br></p>".to_owned();
        assert!(is_untouched(&d), "a blank body is untouched");
        d.body_html = "<p>Hi</p>".to_owned();
        assert!(!is_untouched(&d) && !discards_on_close(&d));
        d.body_html = String::new();
        d.to.push(Addr::new("carol@example.org"));
        assert!(!is_untouched(&d), "a recipient added");
    }

    #[test]
    fn closing_the_first_composer_ends_fresh_and_keeps_the_defaults() {
        let mut d = fresh();
        assert!(composer_closed(&mut d));
        assert!(!is_fresh(&d));
        assert!(is_untouched(&d), "still as it started");
        assert!(!discards_on_close(&d), "only a fresh draft is discarded");
        for mut other in [opened(), kept()] {
            let before = other.lifecycle.clone();
            assert!(!composer_closed(&mut other));
            assert_eq!(other.lifecycle, before);
        }
    }

    #[test]
    fn a_kept_draft_is_never_untouched() {
        for mut d in [fresh(), opened(), kept()] {
            keep(&mut d);
            assert_eq!(d.lifecycle.stage, DraftStage::Kept);
            assert!(!is_fresh(&d) && !is_untouched(&d) && !discards_on_close(&d));
        }
    }

    #[test]
    fn a_landed_body_ends_the_wait_and_its_attachments_count_as_defaults() {
        for mut d in [fresh(), opened()] {
            await_target_body(&mut d);
            assert!(awaits_target_body(&d) && !awaits_parts(&d));
            let added = [attachment("a1")];
            body_landed(&mut d, &added);
            d.attachments.extend(added);
            assert!(!awaits_target_body(&d));
            assert!(is_untouched(&d), "an untouched forward stays untouched");
        }
        let mut d = kept();
        await_target_body(&mut d);
        body_landed(&mut d, &[attachment("a1")]);
        assert_eq!(d.lifecycle.stage, DraftStage::Kept);
        assert_eq!(d.lifecycle.waits_for, None);
    }

    #[test]
    fn a_wait_ends_only_for_what_it_waits_for() {
        let mut d = fresh();
        assert!(!target_body_gone(&mut d) && !parts_downloaded(&mut d));
        await_target_body(&mut d);
        assert!(!parts_downloaded(&mut d), "it waits for a body, not parts");
        assert!(target_body_gone(&mut d));
        assert_eq!(d.lifecycle.waits_for, None);
        read_from_gmail(&mut d, ids("1"), true);
        assert!(awaits_parts(&d));
        assert!(!target_body_gone(&mut d), "it waits for parts, not a body");
        assert!(parts_downloaded(&mut d));
        assert_eq!(d.lifecycle.waits_for, None);
    }

    #[test]
    fn a_create_in_question_is_recovered_until_a_write_answers() {
        let mut d = kept();
        assert!(begin_create(&mut d));
        assert!(!begin_create(&mut d), "already recorded");
        assert_eq!(gmail_write(&d), GmailWrite::Recover);
        written(&mut d, ids("1"));
        assert!(!d.lifecycle.create_sent);
        assert_eq!(
            gmail_write(&d),
            GmailWrite::Update(&GmailDraftId::from("r1"))
        );
        // Gmail no longer had it: the update became a create.
        assert!(begin_create(&mut d));
        assert_eq!(
            gmail_write(&d),
            GmailWrite::Update(&GmailDraftId::from("r1")),
            "the stored id is tried first"
        );
    }

    #[test]
    fn a_gmail_draft_gone_while_open_loses_its_ids_but_not_its_thread() {
        let mut d = opened();
        written(&mut d, ids("1"));
        gmail_gone(&mut d);
        assert_eq!(gmail_draft(&d), None);
        assert_eq!(gmail_message(&d), None);
        assert_eq!(gmail_thread(&d), Some(&GmailThreadId::from("t1")));
        assert_eq!(gmail_write(&d), GmailWrite::Create);
        assert!(!is_fresh(&d) && is_untouched(&d), "its stage stays");
    }

    #[test]
    fn a_skipped_gmail_change_is_read_unless_an_edit_wins() {
        let mut d = opened();
        assert!(gmail_changed(&mut d));
        assert!(!gmail_changed(&mut d), "already skipped");
        assert!(has_skipped_gmail_change(&d));
        edited(&mut d);
        assert!(!has_skipped_gmail_change(&d));
        gmail_changed(&mut d);
        read_from_gmail(&mut d, ids("2"), false);
        assert!(!has_skipped_gmail_change(&d), "read now");
    }

    #[test]
    fn a_read_from_gmail_takes_its_ids_and_waits_for_parts_only_with_attachments() {
        let mut d = fresh();
        await_target_body(&mut d);
        begin_create(&mut d);
        read_from_gmail(&mut d, ids("3"), false);
        assert_eq!(d.lifecycle.waits_for, None, "Gmail's content replaced it");
        assert!(!d.lifecycle.create_sent);
        assert_eq!(gmail_draft(&d), Some(&GmailDraftId::from("r3")));
        assert_eq!(gmail_message(&d), Some(&GmailMessageId::from("m3")));
        assert!(is_fresh(&d), "the stage is the composer's, not Gmail's");
        read_from_gmail(&mut d, ids("4"), true);
        assert!(awaits_parts(&d));
    }
}
