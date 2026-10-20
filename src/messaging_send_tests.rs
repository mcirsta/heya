//! Real SQLCipher saves; the UI owns the one-pending-submission gate.
use super::*;
use crate::archive::tests::{PN, fixture};

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
