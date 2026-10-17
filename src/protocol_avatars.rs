//! SDK picture routing and bounded HTTPS download; no toolkit types or logging.
use crate::{
    archive::AvatarEntry,
    avatars::{self, FetchError, Outcome, Source},
};
use std::{io::Read, sync::Arc, time::Duration};
use whatsapp_rust::wacore_binary::JidExt;
use whatsapp_rust::{
    Client, ContactError, ProfilePictureLookup, ProfilePictureRequest, ProfilePictureTarget,
    ProfilePictureType, request::IqError, wacore_binary::Jid,
};

pub struct Pictures {
    client: Arc<Client>,
    http: ureq::Agent,
}
impl Pictures {
    pub fn new(client: Arc<Client>) -> Self {
        Self {
            client,
            http: ureq::Agent::config_builder()
                .https_only(true)
                .max_redirects(0)
                .timeout_global(Some(Duration::from_secs(8)))
                .build()
                .new_agent(),
        }
    }
}
// A cancelled blocking HTTP read can finish after disconnect. Keep its global
// slot until it actually exits; reconnects cannot accumulate download threads.
static DOWNLOAD: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

fn allowed_url(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| {
            ["whatsapp.net", "whatsapp.com", "fbcdn.net", "fbsbx.com"]
                .iter()
                .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
        })
}

fn download(agent: &ureq::Agent, url: &str) -> Result<avatars::Thumbnail, FetchError> {
    if !allowed_url(url) {
        return Err(FetchError::Retry);
    }
    let mut response = agent.get(url).call().map_err(|error| match error {
        ureq::Error::StatusCode(429) => FetchError::RateLimited(Duration::from_secs(300)),
        _ => FetchError::Retry,
    })?;
    if !response.status().is_success() {
        return Err(FetchError::Retry);
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take((avatars::MAX_DOWNLOAD + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| FetchError::Retry)?;
    avatars::decode(&bytes).ok_or(FetchError::Retry)
}

#[async_trait::async_trait]
impl Source for Pictures {
    async fn fetch(&self, entry: &AvatarEntry) -> Result<Outcome, FetchError> {
        let permit = DOWNLOAD.acquire().await.map_err(|_| FetchError::Retry)?;
        let jid: Jid = entry.jid.parse().map_err(|_| FetchError::Retry)?;
        if !jid.is_group() && !jid.is_pn() && !jid.is_lid() {
            return Ok(Outcome::Absent);
        }
        let existing = entry
            .has_photo
            .then_some(entry.picture_id.as_deref())
            .flatten();
        let device = self.client.persistence_manager().get_device_snapshot();
        let mut candidates = vec![jid.clone()];
        if device
            .pn
            .as_ref()
            .is_some_and(|own| own.to_non_ad() == jid.to_non_ad())
            && let Some(lid) = &device.lid
            && lid.to_non_ad() != jid.to_non_ad()
        {
            candidates.push(lid.to_non_ad());
        }
        // Some accounts supply only the full picture. Do not retry a privacy
        // denial via another route; only an absent preview permits this fallback.
        for candidate in &candidates {
            let target = if candidate.is_group() {
                ProfilePictureTarget::Group(candidate)
            } else {
                ProfilePictureTarget::Contact(candidate)
            };
            for size in [ProfilePictureType::Preview, ProfilePictureType::Full] {
                let request = ProfilePictureRequest::new(target, size)
                    .existing_id(existing)
                    .timeout(Some(Duration::from_secs(8)));
                let outcome = self
                    .client
                    .pictures()
                    .lookup(request)
                    .await
                    .map_err(|error| match error {
                        ContactError::Iq(IqError::ServerError {
                            code: 429, backoff, ..
                        }) => FetchError::RateLimited(Duration::from_secs(
                            backoff.unwrap_or(300) as u64
                        )),
                        _ => FetchError::Retry,
                    })?;
                match outcome {
                    ProfilePictureLookup::Found(picture) => {
                        let http = self.http.clone();
                        return tokio::task::spawn_blocking(move || {
                            let _permit = permit;
                            download(&http, &picture.url).map(|photo| Outcome::Photo {
                                id: picture.id,
                                photo,
                            })
                        })
                        .await
                        .map_err(|_| FetchError::Retry)?;
                    }
                    ProfilePictureLookup::Unchanged if entry.has_photo => {
                        return Ok(Outcome::Unchanged);
                    }
                    ProfilePictureLookup::NotAuthorized => return Ok(Outcome::Absent),
                    ProfilePictureLookup::NotFound => {}
                    _ => return Err(FetchError::Retry),
                }
            }
        }
        Ok(Outcome::Absent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picture_urls_are_https_cdn_addresses_only() {
        assert!(allowed_url("https://mmg.whatsapp.net/synthetic-photo"));
        assert!(allowed_url(
            "https://scontent.example.fbcdn.net/synthetic-photo"
        ));
        for url in [
            "http://mmg.whatsapp.net/p",
            "https://127.0.0.1/p",
            "https://whatsapp.net.evil.invalid/p",
            "https://user@mmg.whatsapp.net/p",
            "https://mmg.whatsapp.net:8443/p",
        ] {
            assert!(!allowed_url(url));
        }
    }
}
