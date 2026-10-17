//! Synthetic SDK/pool lifetimes; never starts a transport or opens an account.
use super::*;
use crate::storage::{prepare_at, tests::MemoryVault};
use whatsapp_rust::store::traits::DeviceStore;

#[tokio::test]
async fn connected_media_sources_release_session_for_repeated_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let vault = MemoryVault::default();
    for close_window in [true, false, true] {
        let store = open_store(prepare_at(temp.path(), &vault).unwrap())
            .await
            .unwrap();
        let bot = Bot::builder().with_backend(store).build().await.unwrap();
        let client = bot.client();
        client
            .persistence_manager()
            .modify_device(|d| d.push_name = "Synthetic shutdown test".into())
            .await;
        let release = client.store_release();
        let (sources, receiver) =
            watch::channel(Some(Arc::new(crate::protocol_audio::Audio(client.clone()))
                as Arc<dyn crate::audio::Source>));
        let (downloads, download_rx) = watch::channel(Some(Arc::new(
            crate::protocol_downloads::Downloads(client.clone()),
        )
            as Arc<dyn crate::downloads::Source>));
        let (uploads, upload_rx) = watch::channel(Some(Arc::new(crate::protocol_uploads::Uploads(
            client.clone(),
        ))
            as Arc<dyn crate::attachments::Uploader>));
        let (history, history_rx) = watch::channel(Some(
            Arc::new(crate::protocol_history::History(client.clone()))
                as Arc<dyn crate::phone_history::Source>,
        ));
        // A closed audio UI still leaves a value owned by the watch sender.
        // Also cover Disconnect, where the audio UI remains alive.
        let receiver = if close_window {
            drop(receiver);
            None
        } else {
            Some(receiver)
        };
        drop(bot);
        let report = client.shutdown().await;
        drop(client);
        assert!(matches!(
            prepare_at(temp.path(), &vault),
            Err(Failure::AlreadyRunning)
        ));
        timeout(
            Duration::from_secs(2),
            finish_shutdown(report, release, &sources, &downloads, &uploads, &history),
        )
        .await
        .expect("published audio source must not deadlock SDK store release")
        .unwrap();
        assert!(history.borrow().is_none());
        assert!(history_rx.borrow().is_none());
        assert!(sources.borrow().is_none());
        assert!(downloads.borrow().is_none());
        assert!(download_rx.borrow().is_none());
        assert!(uploads.borrow().is_none());
        assert!(upload_rx.borrow().is_none());
        if let Some(receiver) = receiver {
            assert!(receiver.borrow().is_none());
        }
        // The pool may drop idle connections on its maintenance worker. The
        // actual OS lock, not just the SDK fence, must become available.
        let files = reopen(temp.path(), &vault).await;
        let store = open_store(files).await.unwrap();
        assert_eq!(
            store.load().await.unwrap().unwrap().push_name,
            "Synthetic shutdown test"
        );
        drop(store);
        drop(reopen(temp.path(), &vault).await);
    }
}

async fn reopen(path: &std::path::Path, vault: &MemoryVault) -> Arc<SessionFiles> {
    timeout(Duration::from_secs(2), async {
        loop {
            match prepare_at(path, vault) {
                Ok(files) => break files,
                Err(Failure::AlreadyRunning) => tokio::time::sleep(Duration::from_millis(10)).await,
                Err(failure) => panic!("unexpected reopen failure: {failure:?}"),
            }
        }
    })
    .await
    .expect("closing must release the session lock")
}
