use sm_domain::{GmailDraftId, GmailMessageId, GmailThreadId};
use sm_store::{Draft, DraftAttachment, DraftStage, DraftWait};

use crate::drafts::text::is_blank;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GmailIds {
    pub(crate) draft: GmailDraftId,
    pub(crate) message: GmailMessageId,
    pub(crate) thread: GmailThreadId,
}

impl GmailIds {
    pub(crate) fn of(written: sm_gmail::Draft) -> Self {
        Self {
            draft: written.id,
            message: written.message.id,
            thread: written.message.thread_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GmailWrite<'d> {
    Update(&'d GmailDraftId),
    Recover,
    Create,
}

pub(crate) fn start_fresh(draft: &mut Draft) {
    let defaults = draft.content().with_body(String::new());
    draft.lifecycle.stage = DraftStage::Fresh { defaults };
}

pub(crate) const fn is_fresh(draft: &Draft) -> bool {
    matches!(draft.lifecycle.stage, DraftStage::Fresh { .. })
}

fn is_untouched(draft: &Draft) -> bool {
    match &draft.lifecycle.stage {
        DraftStage::Fresh { defaults } | DraftStage::Opened { defaults } => {
            defaults.is_untouched_by(draft.content(), is_blank(&draft.body_html))
        }
        DraftStage::Kept => false,
    }
}

pub(crate) fn discards_on_close(draft: &Draft) -> bool {
    is_fresh(draft) && is_untouched(draft)
}

pub(crate) fn composer_closed(draft: &mut Draft) -> bool {
    let DraftStage::Fresh { defaults } = &mut draft.lifecycle.stage else {
        return false;
    };
    let defaults = std::mem::take(defaults);
    draft.lifecycle.stage = DraftStage::Opened { defaults };
    true
}

pub(crate) fn keep(draft: &mut Draft) {
    draft.lifecycle.stage = DraftStage::Kept;
}

pub(crate) const fn await_target_body(draft: &mut Draft) {
    draft.lifecycle.waits_for = Some(DraftWait::TargetBody);
}

pub(crate) fn awaits_target_body(draft: &Draft) -> bool {
    draft.lifecycle.waits_for == Some(DraftWait::TargetBody)
}

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

pub(crate) fn target_body_gone(draft: &mut Draft) -> bool {
    let waited = awaits_target_body(draft);
    if waited {
        draft.lifecycle.waits_for = None;
    }
    waited
}

pub(crate) fn awaits_parts(draft: &Draft) -> bool {
    draft.lifecycle.waits_for == Some(DraftWait::Parts)
}

pub(crate) fn parts_downloaded(draft: &mut Draft) -> bool {
    let waited = awaits_parts(draft);
    if waited {
        draft.lifecycle.waits_for = None;
    }
    waited
}

pub(crate) const fn gmail_draft(draft: &Draft) -> Option<&GmailDraftId> {
    draft.lifecycle.gmail_draft_id.as_ref()
}

pub(crate) const fn gmail_message(draft: &Draft) -> Option<&GmailMessageId> {
    draft.lifecycle.gmail_message_id.as_ref()
}

pub(crate) const fn gmail_thread(draft: &Draft) -> Option<&GmailThreadId> {
    draft.lifecycle.gmail_thread_id.as_ref()
}

pub(crate) const fn gmail_write(draft: &Draft) -> GmailWrite<'_> {
    match gmail_draft(draft) {
        Some(id) => GmailWrite::Update(id),
        None if draft.lifecycle.create_sent => GmailWrite::Recover,
        None => GmailWrite::Create,
    }
}

pub(crate) const fn begin_create(draft: &mut Draft) -> bool {
    let changed = !draft.lifecycle.create_sent;
    draft.lifecycle.create_sent = true;
    changed
}

pub(crate) fn written(draft: &mut Draft, ids: GmailIds) {
    let life = &mut draft.lifecycle;
    life.gmail_draft_id = Some(ids.draft);
    life.gmail_message_id = Some(ids.message);
    life.gmail_thread_id = Some(ids.thread);
    life.create_sent = false;
}

pub(crate) fn gmail_gone(draft: &mut Draft) {
    draft.lifecycle.gmail_draft_id = None;
    draft.lifecycle.gmail_message_id = None;
}

pub(crate) const fn gmail_changed(draft: &mut Draft) -> bool {
    let changed = !draft.lifecycle.gmail_change_skipped;
    draft.lifecycle.gmail_change_skipped = true;
    changed
}

pub(crate) const fn has_skipped_gmail_change(draft: &Draft) -> bool {
    draft.lifecycle.gmail_change_skipped
}

pub(crate) const fn edited(draft: &mut Draft) {
    draft.lifecycle.gmail_change_skipped = false;
}

pub(crate) fn read_from_gmail(draft: &mut Draft, ids: GmailIds, has_parts: bool) {
    draft.lifecycle.waits_for = has_parts.then_some(DraftWait::Parts);
    written(draft, ids);
    draft.lifecycle.gmail_change_skipped = false;
}

#[cfg(test)]
mod tests {
    use sm_domain::{AccountId, Addr, DraftId, DraftKind, Millis};
    use sm_store::DraftLifecycle;

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
