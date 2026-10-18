//! Explicit settings operations; idle is an async wait, not an OS thread.
use crate::{
    AppWindow, audio_devices,
    downloads::{Control, Error},
    media_process,
    voice_recorder::{self, Progress},
};
use slint::ComponentHandle;
use std::{future::Future, path::PathBuf, pin::Pin, time::Duration};
use tokio::sync::{mpsc, watch};
pub enum Command {
    Devices { demo: bool },
    Folder { parent: String, current: PathBuf },
    DefaultFolder,
}
enum Completed {
    Devices(Result<Vec<audio_devices::Device>, voice_recorder::Error>),
    Folder(Result<Option<PathBuf>, Error>),
}
type Job = Pin<Box<dyn Future<Output = Completed> + Send>>;
pub fn attach(
    ui: &AppWindow,
    runtime: &tokio::runtime::Runtime,
) -> (mpsc::Sender<Command>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(4);
    (tx, runtime.spawn(run(ui.as_weak(), rx)))
}
async fn run(weak: slint::Weak<AppWindow>, mut commands: mpsc::Receiver<Command>) {
    let mut job: Option<Job> = None;
    let mut control = Control::default();
    loop {
        tokio::select! {
            result = async {
                match &mut job {Some(j)=>j.await,None=>std::future::pending().await}
            } => {
                job=None;
                let _=weak.upgrade_in_event_loop(move|ui|publish(&ui,result));
            }
            command = commands.recv() => {
                let Some(command)=command else {
                    control.cancel();
                    if let Some(job)=job {let _=tokio::time::timeout(Duration::from_secs(3),job).await;}
                    return;
                };
                if job.is_some(){continue;}
                control=Control::default();
                job=Some(Box::pin(execute(command,control.clone())));
            }
        }
    }
}
async fn execute(command: Command, cancel: Control) -> Completed {
    match command {
        Command::Devices { demo } => {
            if demo || cfg!(test) {
                return Completed::Devices(Ok(vec![
                    audio_devices::Device {
                        id: "synthetic-input".into(),
                        name: "Synthetic microphone".into(),
                        input: true,
                    },
                    audio_devices::Device {
                        id: "synthetic-output".into(),
                        name: "Synthetic speakers".into(),
                        input: false,
                    },
                ]));
            }
            Completed::Devices(discover(cancel).await)
        }
        Command::Folder { parent, current } => {
            Completed::Folder(crate::downloads_picker::folder(&parent, &current, &cancel).await)
        }
        Command::DefaultFolder => Completed::Folder(
            crate::local_worker::run(crate::preferences::default_download_directory)
                .await
                .map(Some)
                .map_err(|_| Error::Destination),
        ),
    }
}
async fn discover(cancel: Control) -> Result<Vec<audio_devices::Device>, voice_recorder::Error> {
    let mut worker = media_process::Worker::start(
        media_process::Mode::Devices,
        watch::channel(Progress::default()).0,
    )?;
    let result = tokio::select! {
        result=worker.finished()=>result,
        _=cancel.cancelled()=>{worker.stop();worker.finished().await},
    }?;
    match result {
        media_process::Outcome::Devices(devices) => Ok(devices),
        _ => Err(voice_recorder::Error::Device),
    }
}
fn publish(ui: &AppWindow, result: Completed) {
    ui.set_settings_io_busy(false);
    match result {
        Completed::Devices(Ok(devices)) => {
            ui.set_audio_devices(slint::ModelRc::new(slint::VecModel::from(
                devices
                    .into_iter()
                    .map(|d| crate::ui::AudioDeviceRow {
                        id: d.id.into(),
                        name: d.name.into(),
                        input: d.input,
                    })
                    .collect::<Vec<_>>(),
            )));
            ui.set_audio_devices_loaded(true);
        }
        Completed::Devices(Err(_)) => ui.set_settings_status(
            if cfg!(target_os = "macos") {
                "Could not list audio devices. Check your connected devices and try Refresh devices."
            } else {
                "Could not list audio devices. Check GStreamer and try Refresh devices."
            }.into(),
        ),
        Completed::Folder(Ok(Some(path))) => {
            ui.invoke_download_folder_ready(path.to_string_lossy().as_ref().into())
        }
        Completed::Folder(Ok(None) | Err(Error::Cancelled)) => {}
        Completed::Folder(Err(_)) => ui.set_settings_status(
            "Could not choose the download folder. Your previous folder is unchanged.".into(),
        ),
    }
    ui.invoke_audio_devices_ready();
}
