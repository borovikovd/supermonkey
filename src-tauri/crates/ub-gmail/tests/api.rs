//! The client's calls against the fake's stateful mailbox.

mod common;

use common::*;
use ub_fake_gmail::{NewAttachment, NewMessage};
use ub_gmail::{Format, ThreadListQuery};

#[tokio::test]
async fn profile_send_as_and_labels() {
    let (_fake, user, gmail) = setup().await;
    user.set_display_name("Me Myself");
    user.add_send_as("alias@example.com", "Alias");
    user.add_label("Work/Projects");

    let profile = gmail.get_profile().await.unwrap();
    assert_eq!(profile.email_address, ME);
    assert_eq!(profile.history_id, user.history_id());

    let send_as = gmail.list_send_as().await.unwrap();
    assert_eq!(send_as.len(), 2);
    assert!(send_as[0].is_primary);
    assert_eq!(send_as[0].display_name, "Me Myself");
    assert_eq!(send_as[1].send_as_email, "alias@example.com");

    let labels = gmail.list_labels().await.unwrap();
    assert!(labels.iter().any(|l| l.id == "INBOX"));
    assert!(
        labels
            .iter()
            .any(|l| l.name == "Work/Projects" && l.id.starts_with("Label_"))
    );
}

#[tokio::test]
async fn threads_list_filters_and_pages() {
    let (_fake, user, gmail) = setup().await;
    let first = user.add_message(NewMessage::inbox("ann@x.com", "Lunch plans", "Noon?"));
    user.add_message(NewMessage {
        thread_id: Some(first.thread_id.clone()),
        in_reply_to: Some(first.rfc822_message_id.clone()),
        ..NewMessage::inbox("bob@x.com", "Re: Lunch plans", "Sure")
    });
    for i in 0..4 {
        user.add_message(NewMessage::inbox(
            "news@x.com",
            &format!("Newsletter {i}"),
            "Read me",
        ));
    }
    user.add_message(NewMessage {
        labels: vec!["TRASH".into()],
        ..NewMessage::inbox("old@x.com", "Lunch leftovers", "Gone")
    });

    let inbox = |page_token: Option<String>| ThreadListQuery {
        label_ids: vec!["INBOX".into()],
        max_results: Some(3),
        page_token,
        ..Default::default()
    };
    let page1 = gmail.list_threads(&inbox(None)).await.unwrap();
    assert_eq!(page1.threads.len(), 3);
    assert_eq!(page1.result_size_estimate, 5);
    assert_eq!(page1.threads[0].snippet, "Read me");
    let page2 = gmail
        .list_threads(&inbox(page1.next_page_token))
        .await
        .unwrap();
    assert_eq!(page2.threads.len(), 2);
    assert!(page2.next_page_token.is_none());
    assert_eq!(page2.threads[1].id, first.thread_id);

    let search = |q: &str, include_spam_trash| ThreadListQuery {
        q: Some(q.into()),
        include_spam_trash,
        ..Default::default()
    };
    let lunch = gmail.list_threads(&search("lunch", false)).await.unwrap();
    assert_eq!(lunch.threads.len(), 1);
    let lunch_all = gmail.list_threads(&search("lunch", true)).await.unwrap();
    assert_eq!(lunch_all.threads.len(), 2);
    let trash = gmail
        .list_threads(&search("in:trash", false))
        .await
        .unwrap();
    assert_eq!(trash.threads.len(), 1);
    let from = gmail
        .list_threads(&search("from:bob in:inbox", false))
        .await
        .unwrap();
    assert_eq!(from.threads[0].id, first.thread_id);
}

#[tokio::test]
async fn thread_formats_and_attachments() {
    let (_fake, user, gmail) = setup().await;
    let added = user.add_message(NewMessage {
        html: Some("<p>Hello <b>there</b></p>".into()),
        list_id: Some("<news.x.com>".into()),
        attachments: vec![NewAttachment {
            filename: "report.pdf".into(),
            mime_type: "application/pdf".into(),
            data: b"%PDF-1.4 fake".to_vec(),
            content_id: None,
        }],
        ..NewMessage::inbox("ann@x.com", "Report", "Hello there")
    });

    let meta = gmail
        .get_thread(&added.thread_id, Format::Metadata, &["Subject", "List-Id"])
        .await
        .unwrap();
    let headers = meta.messages[0].payload.as_ref().unwrap()["headers"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(headers.len(), 2);
    assert_eq!(meta.messages[0].label_ids, vec!["INBOX", "UNREAD"]);
    assert!(meta.messages[0].internal_date.is_some());

    let full = gmail.get_message(&added.id, Format::Full).await.unwrap();
    let payload = full.payload.unwrap();
    assert_eq!(payload["mimeType"], "multipart/mixed");
    let attachment = &payload["parts"][1];
    assert_eq!(attachment["filename"], "report.pdf");
    let attachment_id = attachment["body"]["attachmentId"].as_str().unwrap();
    let bytes = gmail
        .get_attachment(&added.id, attachment_id)
        .await
        .unwrap();
    assert_eq!(bytes, b"%PDF-1.4 fake");

    let raw = gmail.get_message(&added.id, Format::Raw).await.unwrap();
    assert!(raw.raw.is_some());
}

#[tokio::test]
async fn label_changes_show_up_in_history() {
    let (_fake, user, gmail) = setup().await;
    let start = gmail.get_profile().await.unwrap().history_id;
    let a = user.add_message(NewMessage::inbox("ann@x.com", "A", "a"));
    let b = user.add_message(NewMessage::inbox("bob@x.com", "B", "b"));

    gmail
        .batch_modify(&[a.id.clone(), b.id.clone()], &[], &["INBOX".into()])
        .await
        .unwrap();
    let trashed = gmail.trash_message(&a.id).await.unwrap();
    assert!(trashed.label_ids.contains(&"TRASH".to_owned()));

    let history = gmail.list_history(start, None).await.unwrap();
    assert_eq!(history.history_id, user.history_id());
    let added: Vec<_> = history
        .history
        .iter()
        .flat_map(|h| &h.messages_added)
        .map(|m| m.message.id.clone())
        .collect();
    assert_eq!(added, vec![a.id.clone(), b.id.clone()]);
    let removed: Vec<_> = history
        .history
        .iter()
        .flat_map(|h| &h.labels_removed)
        .map(|c| (c.message.id.clone(), c.label_ids.clone()))
        .collect();
    assert_eq!(
        removed,
        vec![
            (a.id.clone(), vec!["INBOX".to_owned()]),
            (b.id.clone(), vec!["INBOX".to_owned()])
        ]
    );
    assert!(
        history
            .history
            .iter()
            .flat_map(|h| &h.labels_added)
            .any(|c| c.message.id == a.id && c.label_ids == ["TRASH"])
    );
}

#[tokio::test]
async fn draft_lifecycle() {
    let (_fake, user, gmail) = setup().await;
    let draft = gmail
        .create_draft(&draft_raw("First", "local-1"), None)
        .await
        .unwrap();
    assert_eq!(draft.message.label_ids, vec!["DRAFT"]);

    let updated = gmail
        .update_draft(&draft.id, &draft_raw("Second", "local-1"), None)
        .await
        .unwrap();
    assert_eq!(updated.id, draft.id);
    assert_ne!(updated.message.id, draft.message.id);
    assert_eq!(updated.message.thread_id, draft.message.thread_id);

    let listed = gmail.list_drafts(None).await.unwrap();
    assert_eq!(listed.drafts.len(), 1);
    assert_eq!(listed.drafts[0].message.id, updated.message.id);

    let full = gmail.get_draft(&draft.id, Format::Full).await.unwrap();
    assert_eq!(full.message.snippet, "Body of Second");

    let sent = gmail.send_draft(&draft.id).await.unwrap();
    assert_eq!(sent.label_ids, vec!["SENT"]);
    assert_eq!(sent.thread_id, draft.message.thread_id);
    assert!(gmail.list_drafts(None).await.unwrap().drafts.is_empty());

    let other = gmail
        .create_draft(&draft_raw("Discard me", "local-2"), None)
        .await
        .unwrap();
    gmail.delete_draft(&other.id).await.unwrap();
    assert!(user.drafts().is_empty());
}
