//! Desktop activation uses the session bus on the existing Tokio runtime.
use std::{collections::HashMap, time::Duration};
use zbus::{Connection, connection::Builder, fdo::RequestNameFlags, zvariant::OwnedValue};

const BUS_NAME: &str = "org.heya.Heya";
const APP_PATH: &str = "/org/heya/Heya";
const INTERFACE: &str = "org.freedesktop.Application";
const IO_TIMEOUT: Duration = Duration::from_secs(2);

type PlatformData = HashMap<String, OwnedValue>;

struct Application<F>(F);

#[zbus::interface(name = "org.freedesktop.Application", spawn = false)]
impl<F: Fn(Option<String>) -> bool + Send + Sync + 'static> Application<F> {
    fn activate(&self, _platform_data: PlatformData) -> zbus::fdo::Result<()> {
        self.deliver(None)
    }

    fn open(&self, uris: Vec<String>, _platform_data: PlatformData) -> zbus::fdo::Result<()> {
        let [uri] = <[_; 1]>::try_from(uris)
            .map_err(|_| zbus::fdo::Error::InvalidArgs("Expected a single WhatsApp link".into()))?;
        self.deliver(Some(uri))
    }
}

impl<F: Fn(Option<String>) -> bool> Application<F> {
    fn deliver(&self, uri: Option<String>) -> zbus::fdo::Result<()> {
        if (self.0)(uri) {
            Ok(())
        } else {
            Err(zbus::fdo::Error::Failed("Heya is busy".into()))
        }
    }
}

pub(crate) enum Start {
    Primary(Option<Connection>),
    Forwarded,
    Rejected,
}

pub(crate) async fn start(
    initial: Option<&str>,
    callback: impl Fn(Option<String>) -> bool + Send + Sync + 'static,
) -> Start {
    match Builder::session() {
        Ok(builder) => register_or_forward(builder, initial, callback).await,
        Err(_) => Start::Primary(None),
    }
}

async fn register_or_forward(
    builder: Builder<'_>,
    initial: Option<&str>,
    callback: impl Fn(Option<String>) -> bool + Send + Sync + 'static,
) -> Start {
    let Ok(builder) = builder
        .method_timeout(IO_TIMEOUT)
        .serve_at(APP_PATH, Application(callback))
    else {
        return Start::Primary(None);
    };
    let Ok(Ok(connection)) = tokio::time::timeout(IO_TIMEOUT, builder.build()).await else {
        return Start::Primary(None);
    };
    let claim_name =
        || connection.request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into());
    match claim_name().await {
        Ok(_) => Start::Primary(Some(connection)),
        Err(zbus::Error::NameTaken) => match forward(&connection, initial).await {
            Ok(()) => Start::Forwarded,
            // The existing process closed before delivery. Reclaim once if the name is free.
            Err(zbus::Error::MethodError(name, _, _))
                if matches!(
                    name.as_str(),
                    "org.freedesktop.DBus.Error.ServiceUnknown"
                        | "org.freedesktop.DBus.Error.NameHasNoOwner"
                ) =>
            {
                if claim_name().await.is_ok() {
                    Start::Primary(Some(connection))
                } else {
                    Start::Rejected
                }
            }
            Err(_) => Start::Rejected,
        },
        Err(_) => Start::Primary(None),
    }
}

async fn forward(connection: &Connection, initial: Option<&str>) -> zbus::Result<()> {
    let platform_data = PlatformData::new();
    if let Some(uri) = initial {
        connection
            .call_method(
                Some(BUS_NAME),
                APP_PATH,
                Some(INTERFACE),
                "Open",
                &(vec![uri], platform_data),
            )
            .await?;
    } else {
        connection
            .call_method(
                Some(BUS_NAME),
                APP_PATH,
                Some(INTERFACE),
                "Activate",
                &(platform_data,),
            )
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use tokio::{
        io::{AsyncBufReadExt, BufReader},
        process::Command,
        sync::mpsc,
    };

    #[tokio::test]
    async fn existing_instance_accepts_links_and_rejects_a_full_inbox() {
        let directory = tempfile::tempdir().unwrap();
        let mut bus = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .arg(format!(
                "--address=unix:path={}",
                directory.path().join("bus").display()
            ))
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut address = String::new();
        tokio::time::timeout(
            IO_TIMEOUT,
            BufReader::new(bus.stdout.take().unwrap()).read_line(&mut address),
        )
        .await
        .unwrap()
        .unwrap();
        let builder = || Builder::address(address.trim()).unwrap();
        let (inbox, mut requests) = mpsc::channel(1);
        let Start::Primary(Some(_primary)) =
            register_or_forward(builder(), None, move |link| inbox.try_send(link).is_ok()).await
        else {
            panic!("Primary instance did not claim the test bus name")
        };

        assert!(matches!(
            register_or_forward(builder(), None, |_| false).await,
            Start::Forwarded
        ));
        assert_eq!(requests.try_recv().unwrap(), None);
        let uri = "whatsapp://send?phone=15550000001&text=Synthetic";
        assert!(matches!(
            register_or_forward(builder(), Some(uri), |_| false).await,
            Start::Forwarded
        ));
        assert!(matches!(
            register_or_forward(builder(), Some(uri), |_| false).await,
            Start::Rejected
        ));
        assert_eq!(requests.try_recv().unwrap().as_deref(), Some(uri));
        bus.kill().await.unwrap();
    }
}
