//! A read-only panel uses latest-value demand and existing media/action paths.
use crate::{
    AppWindow,
    contact_info::{self, Demand, Kind, Section, Snapshot},
    photos,
    ui::ContactInfoMedia,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::sync::Arc;
use tokio::sync::watch;

#[cfg(test)]
#[path = "presentation_contact_info_tests.rs"]
pub(crate) mod tests;

fn next_request(ui: &AppWindow) -> i32 {
    let next = ui.get_contact_info_request().wrapping_add(1).max(1);
    ui.set_contact_info_request(next);
    next
}

fn clear(ui: &AppWindow) {
    ui.set_contact_info_chat(0);
    ui.set_contact_info_media(Default::default());
    ui.set_contact_info_items(Default::default());
    ui.set_contact_info_name("".into());
    ui.set_contact_info_phone("".into());
    ui.set_contact_info_ephemeral("".into());
    ui.set_contact_info_management_status("".into());
    ui.set_contact_info_error("".into());
    ui.set_contact_info_media_count(0);
    ui.set_contact_info_more(false);
    ui.set_contact_info_loading(false);
    ui.set_contact_info_group(false);
    ui.set_contact_info_own(false);
}

pub(crate) fn bind(ui: &AppWindow) -> watch::Receiver<Demand> {
    let (requests, receiver) = watch::channel(Demand::default());
    ui.on_show_contact_info({
        let weak = ui.as_weak();
        let requests = requests.clone();
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if ui.get_selected_id() <= 0 {
                return;
            }
            clear(&ui);
            let request = next_request(&ui);
            ui.set_contact_info_chat(ui.get_selected_id());
            ui.set_contact_info_media_view(false);
            ui.set_contact_info_first(0);
            ui.set_contact_info_name(ui.get_chat_title());
            ui.set_contact_info_group(ui.get_chat_is_group());
            ui.set_contact_info_loading(true);
            ui.set_contact_info_open(true);
            requests.send_replace(Demand {
                chat: ui.get_selected_id(),
                request,
                ..Default::default()
            });
        }
    });
    ui.on_close_contact_info({
        let weak = ui.as_weak();
        let requests = requests.clone();
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let request = next_request(&ui);
            ui.set_contact_info_open(false);
            clear(&ui);
            requests.send_replace(Demand {
                request,
                ..Default::default()
            });
            ui.invoke_request_photos();
        }
    });
    ui.on_request_contact_media({
        let weak = ui.as_weak();
        move |tab| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !ui.get_contact_info_open() || ui.get_selected_id() <= 0 {
                return;
            }
            let section = match tab {
                -1 => None,
                0 => Some(Section::Media),
                1 => Some(Section::Links),
                2 => Some(Section::Documents),
                _ => return,
            };
            let request = next_request(&ui);
            ui.set_contact_info_items(Default::default());
            ui.set_contact_info_error("".into());
            ui.set_contact_info_first(0);
            ui.set_contact_info_loading(true);
            ui.set_contact_info_tab(tab.max(0));
            ui.set_contact_info_media_view(tab >= 0);
            requests.send_replace(Demand {
                chat: ui.get_selected_id(),
                request,
                section,
                media: tab >= 0,
            });
            ui.invoke_request_photos();
        }
    });
    ui.on_open_contact_media({
        let weak = ui.as_weak();
        move |id, kind| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let Some(row) = item(&ui, id).filter(|row| row.kind == kind) else {
                return;
            };
            match kind {
                0 => ui.invoke_activate_photo(id),
                1 => ui.invoke_video_control(id, 0, 0.),
                2 => ui.invoke_download_message(ui.get_selected_id(), id),
                3 if !row.url.is_empty() => ui.invoke_activate_message(id, -1),
                _ => {}
            }
        }
    });
    receiver
}

pub(crate) fn item(ui: &AppWindow, id: i32) -> Option<ContactInfoMedia> {
    if !ui.get_contact_info_open() || ui.get_contact_info_chat() != ui.get_selected_id() || id <= 0
    {
        return None;
    }
    ui.get_contact_info_media()
        .iter()
        .chain(ui.get_contact_info_items().iter())
        .find(|row| row.id == id)
}

pub(crate) fn has(ui: &AppWindow, id: i32, kind: Kind) -> bool {
    item(ui, id).is_some_and(|row| row.kind == kind as i32)
}

/// Share the photo service's existing bounded preview demand and texture cache.
pub(crate) fn photo_demand(ui: &AppWindow) -> Vec<i32> {
    if !ui.get_contact_info_open() || ui.get_contact_info_chat() != ui.get_selected_id() {
        return Vec::new();
    }
    if ui.get_contact_info_media_view() {
        ui.get_contact_info_items()
            .iter()
            .skip(ui.get_contact_info_first().max(0) as usize)
            .take(ui.get_contact_info_count().clamp(0, 16) as usize)
            .filter(|row| row.kind == Kind::Photo as i32)
            .map(|row| row.id)
            .collect()
    } else {
        ui.get_contact_info_media()
            .iter()
            .take(contact_info::PREVIEW_COUNT)
            .map(|row| row.id)
            .collect()
    }
}

fn row(
    id: i32,
    kind: Kind,
    label: String,
    detail: String,
    url: Option<String>,
) -> ContactInfoMedia {
    ContactInfoMedia {
        id,
        kind: kind as i32,
        label: label.into(),
        detail: detail.into(),
        url: url.unwrap_or_default().into(),
        thumbnail: Default::default(),
    }
}

pub(crate) fn publish(ui: &AppWindow, snapshot: &Arc<Snapshot>) {
    let demand = &snapshot.demand;
    if !ui.get_contact_info_open()
        || demand.chat != ui.get_selected_id()
        || demand.request != ui.get_contact_info_request()
    {
        return;
    }
    ui.set_contact_info_loading(false);
    ui.set_contact_info_error(snapshot.error.as_str().into());
    let Some(info) = snapshot.info.as_ref().filter(|i| i.chat == demand.chat) else {
        clear(ui);
        ui.set_contact_info_chat(demand.chat);
        ui.set_contact_info_error(snapshot.error.as_str().into());
        ui.invoke_request_photos();
        return;
    };
    ui.set_contact_info_chat(info.chat);
    ui.set_contact_info_name(info.name.as_str().into());
    ui.set_contact_info_phone(info.phone.as_deref().unwrap_or("").into());
    ui.set_contact_info_group(info.group);
    ui.set_contact_info_own(info.own);
    ui.set_contact_info_muted(info.muted);
    ui.set_contact_info_archived(info.archived);
    ui.set_contact_info_ephemeral(
        match info.ephemeral_seconds {
            0 => String::new(),
            86_400 => "24 hours".into(),
            604_800 => "7 days".into(),
            7_776_000 => "90 days".into(),
            seconds => format!("{} hours", seconds.div_ceil(3600)),
        }
        .into(),
    );
    ui.set_contact_info_media_count(info.shared_count.min(i32::MAX as usize) as i32);
    let previews = info
        .photos
        .iter()
        .take(contact_info::PREVIEW_COUNT)
        .map(|&id| row(id, Kind::Photo, "Photo".into(), String::new(), None))
        .collect::<Vec<_>>();
    replace(ui.get_contact_info_media(), previews, |model| {
        ui.set_contact_info_media(model)
    });
    if let Some(list) = snapshot
        .list
        .as_ref()
        .filter(|l| l.chat == demand.chat && l.section == demand.section)
    {
        let rows = list
            .rows
            .iter()
            .take(contact_info::MEDIA_LIMIT)
            .map(|m| {
                row(
                    m.id,
                    m.kind,
                    m.label.clone(),
                    m.detail.clone(),
                    m.link.clone(),
                )
            })
            .collect::<Vec<_>>();
        replace(ui.get_contact_info_items(), rows, |model| {
            ui.set_contact_info_items(model)
        });
        ui.set_contact_info_more(list.more);
    } else {
        ui.set_contact_info_items(Default::default());
        ui.set_contact_info_more(false);
    }
    refresh_images(ui);
    ui.invoke_request_photos();
}

fn replace(
    old: ModelRc<ContactInfoMedia>,
    mut rows: Vec<ContactInfoMedia>,
    set: impl FnOnce(ModelRc<ContactInfoMedia>),
) {
    for row in &mut rows {
        if let Some(previous) = old
            .iter()
            .find(|previous| previous.id == row.id && previous.kind == row.kind)
        {
            row.thumbnail = previous.thumbnail;
        }
    }
    if old.row_count() != rows.len()
        || old.iter().zip(&rows).any(|(a, b)| {
            a.id != b.id
                || a.kind != b.kind
                || a.label != b.label
                || a.detail != b.detail
                || a.url != b.url
        })
    {
        set(ModelRc::new(VecModel::from(rows)));
    }
}

fn refresh_images(ui: &AppWindow) {
    if !ui.get_contact_info_open() {
        return;
    }
    for rows in [ui.get_contact_info_media(), ui.get_contact_info_items()] {
        let Some(model) = rows.as_any().downcast_ref::<VecModel<ContactInfoMedia>>() else {
            continue;
        };
        for i in 0..model.row_count() {
            let mut row = model.row_data(i).unwrap();
            if row.kind != Kind::Photo as i32 {
                continue;
            }
            let next = crate::presentation::timeline::cached_photo(ui, row.id);
            if !(next.size().width == 0 && row.thumbnail.size().width == 0) && row.thumbnail != next
            {
                row.thumbnail = next;
                model.set_row_data(i, row);
            }
        }
    }
}

pub(crate) fn publish_photos(ui: &AppWindow, _snapshot: &Arc<photos::Snapshot>) {
    refresh_images(ui);
}

pub(crate) fn attach(
    ui: &AppWindow,
    archives: watch::Receiver<Option<crate::archive::Archive>>,
) -> impl std::future::Future<Output = ()> + Send + use<> {
    let requests = bind(ui);
    let (updates, snapshots) = watch::channel(Arc::new(Snapshot::default()));
    let weak = ui.as_weak();
    async move {
        tokio::join!(
            contact_info::run(archives, requests, updates),
            crate::live_presentation::present_latest(weak, snapshots, publish)
        );
    }
}

pub(crate) fn attach_demo(ui: &AppWindow) -> impl std::future::Future<Output = ()> + Send + use<> {
    let mut requests = bind(ui);
    let (updates, snapshots) = watch::channel(Arc::new(Snapshot::default()));
    let weak = ui.as_weak();
    let demo = crate::fixtures::demo();
    async move {
        tokio::join!(
            async move {
                while requests.changed().await.is_ok() {
                    let demand = requests.borrow_and_update().clone();
                    let mut snapshot = Snapshot {
                        demand: demand.clone(),
                        ..Default::default()
                    };
                    if let Some(chat) = demo.chats.iter().find(|c| c.id == demand.chat) {
                        let mut info = contact_info::Info {
                            chat: chat.id,
                            name: chat.title.clone(),
                            phone: (!chat.is_group).then(|| "+1 555 000 0001".into()),
                            group: chat.is_group,
                            ..Default::default()
                        };
                        let mut items = Vec::new();
                        for message in chat.messages.iter().rev() {
                            let content = &message.content;
                            let kind = if content.photo.is_some() {
                                info.media_count += 1;
                                info.photos.push(message.id);
                                Some(Kind::Photo)
                            } else if content.video.is_some() {
                                info.media_count += 1;
                                Some(Kind::Video)
                            } else if content.document.is_some() {
                                info.document_count += 1;
                                Some(Kind::Document)
                            } else if content.preview.is_some() {
                                info.link_count += 1;
                                Some(Kind::Link)
                            } else {
                                None
                            };
                            if let Some(kind) = kind {
                                info.shared_count += 1;
                                let label = content
                                    .document
                                    .as_ref()
                                    .map_or_else(|| message.text.clone(), |d| d.file_name.clone());
                                items.push(contact_info::Media {
                                    id: message.id,
                                    kind,
                                    label,
                                    detail: message.time.clone(),
                                    link: content.preview.as_ref().map(|p| p.url.clone()),
                                });
                            }
                        }
                        info.photos.truncate(contact_info::PREVIEW_COUNT);
                        if demand.media {
                            items.retain(|m| match demand.section {
                                None => true,
                                Some(Section::Media) => matches!(m.kind, Kind::Photo | Kind::Video),
                                Some(Section::Links) => m.kind == Kind::Link,
                                Some(Section::Documents) => m.kind == Kind::Document,
                            });
                            let more = items.len() > contact_info::MEDIA_LIMIT;
                            items.truncate(contact_info::MEDIA_LIMIT);
                            snapshot.list = Some(contact_info::MediaList {
                                chat: chat.id,
                                section: demand.section,
                                rows: items,
                                more,
                            });
                        }
                        snapshot.info = Some(info);
                    }
                    if updates.send(Arc::new(snapshot)).is_err() {
                        break;
                    }
                }
            },
            crate::live_presentation::present_latest(weak, snapshots, publish)
        );
    }
}
