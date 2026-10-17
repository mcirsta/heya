//! A private D-Bus service exercises the actual portal wire contract, no desktop UI.
use super::*;
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
};
use zbus::{
    Connection,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

struct Chooser {
    mode: Arc<AtomicU32>,
    opened: mpsc::Sender<(String, String, bool)>,
    closed: mpsc::Sender<()>,
}
struct Request {
    closed: mpsc::Sender<()>,
}
#[zbus::interface(name = "org.freedesktop.portal.Request")]
impl Request {
    async fn close(&self) {
        let _ = self.closed.send(()).await;
    }
}
#[zbus::interface(name = "org.freedesktop.portal.FileChooser")]
impl Chooser {
    async fn open_file(
        &self,
        parent: &str,
        title: &str,
        mut options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let folder = self.mode.load(Ordering::SeqCst) == 6;
        assert_eq!(
            bool::try_from(options.get("multiple").unwrap()).unwrap(),
            !folder
        );
        assert_eq!(
            bool::try_from(options.get("directory").unwrap()).unwrap(),
            folder
        );
        if folder {
            assert_eq!(
                Vec::<u8>::try_from(options.get("current_folder").unwrap().try_clone().unwrap())
                    .unwrap(),
                b"/tmp/Heya\0"
            );
        }
        assert_eq!(
            options.contains_key("filters"),
            self.mode.load(Ordering::SeqCst) == 4
        );
        options.insert(
            "current_name".into(),
            Value::from("Open fixture").try_to_owned().unwrap(),
        );
        self.save_file(parent, title, options, header, connection)
            .await
    }
    async fn save_file(
        &self,
        parent: &str,
        _title: &str,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let token = <&str>::try_from(options.get("handle_token").unwrap()).unwrap();
        let name = <&str>::try_from(options.get("current_name").unwrap()).unwrap();
        let modal = bool::try_from(options.get("modal").unwrap()).unwrap();
        if self.mode.load(Ordering::SeqCst) < 4 {
            assert_eq!(
                Vec::<u8>::try_from(options.get("current_folder").unwrap().try_clone().unwrap())
                    .unwrap(),
                b"/tmp/Heya\0"
            );
        }
        let sender = header
            .sender()
            .unwrap()
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");
        let path = OwnedObjectPath::try_from(format!(
            "/org/freedesktop/portal/desktop/request/{sender}/{token}"
        ))
        .unwrap();
        connection
            .object_server()
            .at(
                path.clone(),
                Request {
                    closed: self.closed.clone(),
                },
            )
            .await
            .unwrap();
        self.opened
            .send((parent.into(), name.into(), modal))
            .await
            .unwrap();
        let mode = self.mode.load(Ordering::SeqCst);
        if mode != 3 {
            let code = if mode == 1 { 1u32 } else { 0 };
            let results = HashMap::from([(
                "uris",
                Value::from(if mode == 6 {
                    vec!["file:///tmp"]
                } else if mode >= 4 {
                    vec![
                        "file:///tmp/Synthetic%20%E4%BD%A0%E5%A5%BD.pdf",
                        "file:///tmp/Synthetic%20photo.png",
                    ]
                } else {
                    vec![if mode == 2 {
                        "https://not-a-local-file.invalid/file"
                    } else {
                        "file:///tmp/Synthetic%20%E4%BD%A0%E5%A5%BD.pdf"
                    }]
                }),
            )]);
            // Deliberately respond before SaveFile returns its handle.
            connection
                .emit_signal(
                    header.sender().map(|s| s.as_str()),
                    path.as_str(),
                    "org.freedesktop.portal.Request",
                    "Response",
                    &(code, results),
                )
                .await
                .unwrap();
        }
        Ok(path)
    }
}

#[tokio::test]
async fn native_portal_handles_early_response_unicode_cancellation_and_close() {
    let temp = tempfile::tempdir().unwrap();
    let mut daemon = tokio::process::Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address"])
        .arg(format!(
            "--address=unix:path={}",
            temp.path().join("bus").display()
        ))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(daemon.stdout.take().unwrap())
        .read_line(&mut line)
        .await
        .unwrap();
    let address = line.trim();
    let (opened, mut observations) = mpsc::channel(8);
    let (closed, mut closes) = mpsc::channel(8);
    let mode = Arc::new(AtomicU32::new(0));
    let server = zbus::connection::Builder::address(address)
        .unwrap()
        .name("org.freedesktop.portal.Desktop")
        .unwrap()
        .serve_at(
            "/org/freedesktop/portal/desktop",
            Chooser {
                mode: mode.clone(),
                opened,
                closed,
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    for choice in 0..4 {
        mode.store(choice, Ordering::SeqCst);
        let client = zbus::connection::Builder::address(address)
            .unwrap()
            .build()
            .await
            .unwrap();
        let control = Control::default();
        let mut response = Box::pin(portal_request(
            &client,
            "Synthetic 你好.pdf",
            "x11:1234",
            Some(std::path::Path::new("/tmp/Heya")),
            &control,
        ));
        if choice == 3 {
            tokio::select! {
                Some((parent,name,modal))=observations.recv()=>{assert_eq!(parent,"x11:1234");assert_eq!(name,"Synthetic 你好.pdf");assert!(modal);}
                result=&mut response=>panic!("chooser completed unexpectedly: {result:?}"),
            }
            control.cancel();
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(3), &mut response)
                    .await
                    .unwrap(),
                Err(Error::Cancelled)
            );
            tokio::time::timeout(std::time::Duration::from_secs(3), closes.recv())
                .await
                .unwrap()
                .unwrap();
        } else {
            let result = tokio::time::timeout(std::time::Duration::from_secs(3), &mut response)
                .await
                .unwrap();
            match choice {
                0 => assert_eq!(result, Ok(Some(PathBuf::from("/tmp/Synthetic 你好.pdf")))),
                1 => assert_eq!(result, Ok(None)),
                _ => assert_eq!(result, Err(Error::Picker)),
            }
            let (_, name, modal) = observations.recv().await.unwrap();
            assert_eq!(name, "Synthetic 你好.pdf");
            assert!(modal);
        }
        drop(response);
        let _ = client.close().await;
    }
    for images in [true, false] {
        mode.store(if images { 4 } else { 5 }, Ordering::SeqCst);
        let client = zbus::connection::Builder::address(address)
            .unwrap()
            .build()
            .await
            .unwrap();
        let paths = portal_choice(
            &client,
            "x11:1234",
            Choice {
                method: "OpenFile",
                title: "Choose attachments",
                name: "",
                images,
                directory: false,
                folder: None,
            },
            &Control::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/tmp/Synthetic 你好.pdf"),
                PathBuf::from("/tmp/Synthetic photo.png")
            ]
        );
        observations.recv().await.unwrap();
        let _ = client.close().await;
    }
    mode.store(6, Ordering::SeqCst);
    let client = zbus::connection::Builder::address(address)
        .unwrap()
        .build()
        .await
        .unwrap();
    let paths = portal_choice(
        &client,
        "x11:1234",
        Choice {
            method: "OpenFile",
            title: "Download folder",
            name: "",
            images: false,
            directory: true,
            folder: Some(std::path::Path::new("/tmp/Heya")),
        },
        &Control::default(),
    )
    .await
    .unwrap();
    assert_eq!(paths, vec![PathBuf::from("/tmp")]);
    let _ = client.close().await;
    let _ = server.close().await;
    daemon.kill().await.unwrap();
    daemon.wait().await.unwrap();
}
