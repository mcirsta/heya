//! Native file choosers: Linux desktop portal and macOS AppKit sheets.
use crate::downloads::{Control, Error, Picker};
use std::path::PathBuf;
pub struct NativePicker(pub tokio::sync::watch::Receiver<crate::preferences::Preferences>);
#[async_trait::async_trait]
impl Picker for NativePicker {
    async fn save(
        &self,
        name: &str,
        parent: &str,
        control: &Control,
    ) -> Result<Option<PathBuf>, Error> {
        if control.is_cancelled() {
            return Err(Error::Cancelled);
        }
        #[cfg(target_os = "linux")]
        {
            let folder = self.0.borrow().download_directory.clone();
            let create = folder.clone();
            crate::local_worker::run(move || std::fs::create_dir_all(create))
                .await
                .map_err(|_| Error::Storage)?
                .map_err(|_| Error::Destination)?;
            portal(name, parent, Some(&folder), control).await
        }
        #[cfg(target_os = "macos")]
        {
            let _ = parent;
            let folder = self.0.borrow().download_directory.clone();
            let create = folder.clone();
            crate::local_worker::run(move || std::fs::create_dir_all(create))
                .await
                .map_err(|_| Error::Storage)?
                .map_err(|_| Error::Destination)?;
            Ok(crate::macos::choose(0, name, Some(&folder), control)
                .await?
                .into_iter()
                .next())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (name, parent);
            Err(Error::Picker)
        }
    }
}
#[cfg(target_os = "linux")]
async fn portal(
    name: &str,
    parent: &str,
    folder: Option<&std::path::Path>,
    control: &Control,
) -> Result<Option<PathBuf>, Error> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|_| Error::Picker)?;
    let result = portal_request(&connection, name, parent, folder, control).await;
    let _ = connection.close().await;
    result
}

#[cfg(target_os = "linux")]
async fn portal_request(
    connection: &zbus::Connection,
    name: &str,
    parent: &str,
    folder: Option<&std::path::Path>,
    control: &Control,
) -> Result<Option<PathBuf>, Error> {
    let paths = portal_choice(
        connection,
        parent,
        Choice {
            method: "SaveFile",
            title: "Save attachment",
            name,
            images: false,
            directory: false,
            folder,
        },
        control,
    )
    .await?;
    Ok(paths.into_iter().next())
}

pub async fn open(images: bool, parent: &str, control: &Control) -> Result<Vec<PathBuf>, Error> {
    #[cfg(target_os = "linux")]
    {
        let connection = zbus::Connection::session()
            .await
            .map_err(|_| Error::Picker)?;
        let result = portal_choice(
            &connection,
            parent,
            Choice {
                method: "OpenFile",
                title: if images {
                    "Choose photos"
                } else {
                    "Choose documents"
                },
                name: "",
                images,
                directory: false,
                folder: None,
            },
            control,
        )
        .await;
        let _ = connection.close().await;
        result
    }
    #[cfg(target_os = "macos")]
    {
        let _ = parent;
        crate::macos::choose(if images { 2 } else { 1 }, "", None, control).await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (images, parent, control);
        Err(Error::Picker)
    }
}
pub async fn folder(
    parent: &str,
    current: &std::path::Path,
    control: &Control,
) -> Result<Option<PathBuf>, Error> {
    #[cfg(target_os = "linux")]
    {
        let connection = zbus::Connection::session()
            .await
            .map_err(|_| Error::Picker)?;
        let result = portal_choice(
            &connection,
            parent,
            Choice {
                method: "OpenFile",
                title: "Choose download folder",
                name: "",
                images: false,
                directory: true,
                folder: Some(current),
            },
            control,
        )
        .await;
        let _ = connection.close().await;
        let path = result?.into_iter().next();
        if let Some(ref selected) = path {
            let check = selected.clone();
            if !crate::local_worker::run(move || check.is_dir())
                .await
                .map_err(|_| Error::Destination)?
            {
                return Err(Error::Destination);
            }
        }
        Ok(path)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = parent;
        Ok(crate::macos::choose(3, "", Some(current), control)
            .await?
            .into_iter()
            .next())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (parent, current, control);
        Err(Error::Picker)
    }
}
#[cfg(target_os = "linux")]
struct Choice<'a> {
    method: &'a str,
    title: &'a str,
    name: &'a str,
    images: bool,
    directory: bool,
    folder: Option<&'a std::path::Path>,
}
#[cfg(target_os = "linux")]
async fn portal_choice(
    connection: &zbus::Connection,
    parent: &str,
    choice: Choice<'_>,
    control: &Control,
) -> Result<Vec<PathBuf>, Error> {
    let Choice {
        method,
        title,
        name,
        images,
        directory,
        folder,
    } = choice;
    use futures_util::StreamExt;
    use std::collections::HashMap;
    use zbus::{
        Proxy,
        zvariant::{OwnedObjectPath, OwnedValue, Value},
    };
    if control.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let mut random = [0; 12];
    getrandom::fill(&mut random).map_err(|_| Error::Picker)?;
    let token = format!(
        "heya_{}",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let sender = connection
        .unique_name()
        .ok_or(Error::Picker)?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let request = Proxy::new(
        connection,
        "org.freedesktop.portal.Desktop",
        path.as_str(),
        "org.freedesktop.portal.Request",
    )
    .await
    .map_err(|_| Error::Picker)?;
    // Subscribe before SaveFile: its Response may precede the method reply.
    let mut responses = request
        .receive_signal("Response")
        .await
        .map_err(|_| Error::Picker)?;
    let chooser = Proxy::new(
        connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.FileChooser",
    )
    .await
    .map_err(|_| Error::Picker)?;
    let mut options = HashMap::from([
        ("handle_token", Value::from(token.as_str())),
        ("current_name", Value::from(name)),
        ("modal", Value::from(!parent.is_empty())),
    ]);
    if let Some(folder) = folder {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let mut path = folder.as_os_str().as_bytes().to_vec();
            path.push(0);
            options.insert("current_folder", Value::from(path));
        }
    }
    if method == "OpenFile" {
        options.remove("current_name");
        options.insert("multiple", Value::from(!directory));
        options.insert("directory", Value::from(directory));
        if images {
            options.insert(
                "filters",
                Value::from(vec![(
                    "JPEG and PNG images",
                    vec![(1u32, "image/jpeg"), (1u32, "image/png")],
                )]),
            );
        }
    }
    let response = async {
        let returned: OwnedObjectPath = chooser
            .call(method, &(parent, title, options))
            .await
            .map_err(|_| Error::Picker)?;
        if returned.as_str() != path {
            return Err(Error::Picker);
        }
        let response = responses.next().await.ok_or(Error::Picker)?;
        let (code, results): (u32, HashMap<String, OwnedValue>) =
            response.body().deserialize().map_err(|_| Error::Picker)?;
        match code {
            0 => {
                let uris = results
                    .get("uris")
                    .ok_or(Error::Picker)?
                    .try_clone()
                    .map_err(|_| Error::Picker)?;
                let uris: Vec<String> = uris.try_into().map_err(|_| Error::Picker)?;
                if uris.is_empty() || uris.len() > if method == "SaveFile" { 1 } else { 100 } {
                    return Err(Error::Picker);
                }
                uris.iter()
                    .map(|uri| {
                        url::Url::parse(uri)
                            .ok()
                            .and_then(|u| u.to_file_path().ok())
                            .filter(|p| p.is_absolute())
                            .ok_or(Error::Picker)
                    })
                    .collect()
            }
            1 => Ok(Vec::new()),
            _ => Err(Error::Picker),
        }
    };
    tokio::select! {
        result = response => result,
        _ = control.cancelled() => {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), request.call::<_,_,()>("Close",&())).await;
            Err(Error::Cancelled)
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "downloads_picker_tests.rs"]
mod tests;
