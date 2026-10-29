//! Real SQLCipher saves; the UI owns the one-pending-submission gate.
use super::*;
use crate::archive::tests::{PN, fixture};

#[tokio::test]
async fn queued_typing_does_not_make_send_wait_for_every_obsolete_draft() {
    let (_temp, db) = fixture();
    let chat = db.create_chat(PN.into()).await.unwrap();
    db.blocking(|c| {
        c.execute_batch(
            "CREATE TEMP TABLE draft_writes (value TEXT);
             CREATE TEMP TRIGGER count_drafts AFTER UPDATE OF draft ON chats
             BEGIN INSERT INTO draft_writes VALUES(NEW.draft); END;",
        )?;
        Ok(())
    })
    .unwrap();
    let (archives, receiver) = watch::channel(None);
    let (commands, rx) = mpsc::channel(256);
    let (updates, mut views) = watch::channel(Snapshot::empty());
    let task = tokio::spawn(run(receiver, rx, updates));
    archives.send_replace(Some(db.clone()));
    views.changed().await.unwrap();

    // One buffered typing burst followed by Send and a genuinely newer draft.
    // These commands must keep their order without committing every prefix.
    let text = "Synthetic multiline 👩🏽‍💻\năâîșț ".repeat(120);
    for version in 1..=200 {
        commands
            .try_send(Command::Draft(chat, format!("{text}{version}"), version))
            .unwrap();
    }
    commands
        .try_send(Command::Send {
            chat,
            text: text.clone(),
            request: 1,
            draft_version: 200,
        })
        .unwrap();
    commands
        .try_send(Command::Draft(chat, "Newer draft".into(), 201))
        .unwrap();
    drop(commands);
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();

    assert!(views.borrow().reply.as_ref().unwrap().saved);
    db.blocking(move |c| {
        let writes: i32 = c.query_row("SELECT count(*) FROM draft_writes", [], |r| r.get(0))?;
        let draft: String =
            c.query_row("SELECT draft FROM chats WHERE id=?1", [chat], |r| r.get(0))?;
        let sent: String =
            c.query_row("SELECT body FROM messages WHERE sender='#self'", [], |r| {
                r.get(0)
            })?;
        assert_eq!(draft, "Newer draft");
        assert_eq!(sent, text);
        assert_eq!(
            writes, 3,
            "one latest draft, the send's atomic clear, then the newer draft"
        );
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn send_without_an_archive_is_rejected_instead_of_leaving_the_ui_pending() {
    let (_archives, receiver) = watch::channel(None);
    let (commands, rx) = mpsc::channel(1);
    let (updates, mut views) = watch::channel(Snapshot::empty());
    let task = tokio::spawn(run(receiver, rx, updates));
    commands
        .send(Command::Send {
            chat: 1,
            text: "Retain this synthetic draft".into(),
            request: 7,
            draft_version: 2,
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), views.changed())
        .await
        .expect("an accepted send must receive an acknowledgement even without an archive")
        .unwrap();
    let snapshot = views.borrow_and_update().clone();
    let reply = snapshot.reply.unwrap();
    assert_eq!(reply.request, 7);
    assert_eq!(reply.chat, 1);
    assert_eq!(reply.text, "Retain this synthetic draft");
    assert!(!reply.saved);
    assert!(snapshot.view.is_none());
    drop(commands);
    task.await.unwrap();
}

#[tokio::test]
async fn send_replies_report_committed_text_and_allow_retry_after_failure() {
    let (_temp, db) = fixture();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let text = "A long synthetic paragraph 👩🏽‍💻 with Romanian diacritics: ăâîșț.\n".repeat(120);
    let (archives, receiver) = watch::channel(None);
    let (commands, rx) = mpsc::channel(16);
    let (updates, mut views) = watch::channel(Snapshot::empty());
    let task = tokio::spawn(run(receiver, rx, updates));
    archives.send_replace(Some(db.clone()));
    views.changed().await.unwrap();
    for request in 1..=3 {
        db.blocking(move |c| {
            c.execute(
                "UPDATE chats SET read_only=?2 WHERE id=?1",
                rusqlite::params![chat, request == 2],
            )?;
            Ok(())
        })
        .unwrap();
        commands
            .send(Command::Send {
                chat,
                text: text.clone(),
                request,
                draft_version: if request == 1 { 1 } else { 2 },
            })
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if views
                    .borrow_and_update()
                    .reply
                    .as_ref()
                    .is_some_and(|r| r.request == request)
                {
                    break;
                }
                views.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let reply = views.borrow().reply.clone().unwrap();
        assert_eq!(reply.saved, request != 2);
        assert_eq!(reply.text, text);
        // A successful reply means the entire text is already durable, even
        // if a receiver has not applied the following conversation snapshot.
        let expected = if request == 3 { 2 } else { 1 };
        let body = text.clone();
        let count: i32 = db
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM messages WHERE sender='#self' AND body=?1",
                    [body],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(count, expected);
    }
    drop(commands);
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    // Replacing/coalescing intermediate snapshots must retain the last reply.
    let final_view = views.borrow();
    assert_eq!(final_view.reply.as_ref().unwrap().request, 3);
    assert!(final_view.reply.as_ref().unwrap().saved);
    assert!(final_view.view.is_some());
}
