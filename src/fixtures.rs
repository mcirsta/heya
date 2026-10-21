//! Synthetic data only. Demo startup never loads account files or connects a client.
use crate::domain::{Chat, DemoState, Message, PAGE_SIZE};

/// Procedural geometry for offline display/renderer checks; never account photos.
pub fn avatar_samples(ids: impl IntoIterator<Item = i32>) -> crate::avatars::Snapshot {
    use std::sync::Arc;
    let samples = ids
        .into_iter()
        .map(|id| {
            let image =
                image::RgbaImage::from_fn(crate::avatars::EDGE, crate::avatars::EDGE, |x, y| {
                    let seed = id.unsigned_abs().wrapping_mul(41);
                    if y > 90 - x / 3 {
                        image::Rgba([32, (80 + seed % 100) as u8, 72, 255])
                    } else if (x as i32 - 85).pow(2) + (y as i32 - 30).pow(2) < 180 {
                        image::Rgba([255, 222, 140, 255])
                    } else {
                        image::Rgba([(40 + seed % 100) as u8, 150, (180 + y / 3) as u8, 255])
                    }
                });
            (
                id.into(),
                Arc::new(crate::avatars::Thumbnail::from_pixels(image.into_raw()).unwrap()),
            )
        })
        .collect();
    Arc::new(samples)
}

pub fn group_avatar_samples(messages: &[Message]) -> crate::avatars::Snapshot {
    use std::collections::BTreeMap;
    // A few generated landscapes stand in for participant photos in offline
    // checks. Leave one sender without a photo to exercise the initials fallback.
    let samples = avatar_samples(0..3);
    let mut pictures = BTreeMap::new();
    for message in messages.iter().filter(|m| !m.outgoing) {
        if pictures.len() == 3 {
            break;
        }
        let key = crate::avatars::Target::Participant(message.sender_id.clone());
        let image = samples[&(pictures.len() as i32).into()].clone();
        pictures.entry(key).or_insert(image);
    }
    pictures.into()
}

/// Procedural video frame: a moving square over a fixed landscape, no files.
pub fn video_pixels(seconds: f64) -> crate::photos::Pixels {
    let (width, height) = (640, 360);
    let x0 = ((seconds * 90.) as u32) % 560;
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        if x >= x0 && x < x0 + 80 && (100..180).contains(&y) {
            image::Rgba([255, 215, 90, 255])
        } else if y > height * 2 / 3 {
            image::Rgba([28, 125, 88, 255])
        } else {
            image::Rgba([60, (110 + y / 5) as u8, 175, 255])
        }
    });
    crate::photos::Pixels {
        width,
        height,
        rgba: image.into_raw(),
    }
}

pub fn demo() -> DemoState {
    let today = chrono::Local::now().date_naive();
    let mut next_id = 1;
    let mut chat = |id, title: &str, initials: &str, tint, unread, lines: &[(&str, &str, bool)]| {
        let messages = lines
            .iter()
            .enumerate()
            .map(|(i, (sender, text, outgoing))| {
                let message = Message {
                    id: next_id,
                    sender: if matches!(id, 2 | 4) && !outgoing {
                        crate::domain::participant_name(sender, false, sender)
                    } else {
                        (*sender).into()
                    },
                    sender_id: if *outgoing {
                        "#self".into()
                    } else {
                        format!("demo:{sender}")
                    },
                    text: (*text).into(),
                    time: format!("10:{:02}", 24 + i),
                    date: Some(today),
                    outgoing: *outgoing,
                    local: false,
                    content: Default::default(),
                };
                next_id += 1;
                message
            })
            .collect();
        Chat {
            id,
            title: title.into(),
            initials: initials.into(),
            tint,
            is_group: matches!(id, 2 | 4),
            unread,
            messages,
            draft: String::new(),
            visible_count: PAGE_SIZE,
        }
    };
    let mut chats = vec![
        chat(
            1,
            "Ada Lovelace",
            "AL",
            0xc6a1e4,
            0,
            &[
                ("Ada", "A little room for better conversations.", false),
                (
                    "You",
                    "Heya is taking shape. Same familiar layout, with a native desktop feel.",
                    true,
                ),
                (
                    "Ada",
                    "Can I select and copy a message? Even with a few lines?\n\nHello, world. 👋\nSalut, lume. · こんにちは世界",
                    false,
                ),
                (
                    "You",
                    "Yes — drag across the text, then copy. The button in the header copies the loaded conversation as a transcript.",
                    true,
                ),
                (
                    "Ada",
                    "Lovely. Let's keep the small things feeling right.",
                    false,
                ),
                (
                    "You",
                    "Try switching chats while writing. Your draft will be waiting when you come back.",
                    true,
                ),
            ],
        ),
        chat(
            2,
            "Rust café",
            "RC",
            0xa1c9df,
            3,
            &[
                ("Mira", "Coffee and a little weekend project? ☕", false),
                (
                    "Noah",
                    "Count me in. I have a few ideas for the video player.",
                    false,
                ),
                (
                    "You",
                    "Let's start with the basics and build from there.",
                    true,
                ),
                ("Mira", "Saturday, 10:30. The usual table.", false),
            ],
        ),
        chat(
            3,
            "Grace Hopper",
            "GH",
            0xe2b294,
            1,
            &[
                (
                    "Grace",
                    "Found the bug. It was smaller than expected.",
                    false,
                ),
                ("You", "The best kind of afternoon.", true),
                ("Grace", "Everything is a little clearer now. ✨", false),
            ],
        ),
        chat(
            4,
            "Weekend plans",
            "WP",
            0xa5d1b6,
            0,
            &[
                (
                    "Alex",
                    "Train at nine, walk by the lake, lunch somewhere small?",
                    false,
                ),
                ("You", "Perfect. I'll bring the camera.", true),
                ("Sam", "And I'll bring the sandwiches. 🥪", false),
            ],
        ),
        chat(
            5,
            "Words around the world",
            "WW",
            0xe1c884,
            0,
            &[
                ("Leila", "مرحبا بالعالم — hello, world!", false),
                ("You", "שלום · Bonjour · Bună · 你好 · 안녕하세요", true),
                (
                    "Leila",
                    "A few tricky characters: e\u{301}, 👨‍👩‍👧‍👦, 🇷🇴, 👩🏽‍💻.\nTry selecting and copying each one.",
                    false,
                ),
            ],
        ),
        chat(6, "Timeline lab", "TL", 0xa7b3de, 0, &[]),
    ];
    // Keep a profile-name + uncaptioned group photo in the ordinary sidebar
    // fixture, alongside the captioned direct photos below.
    let photo = chats[3].messages.last_mut().unwrap();
    fill_photos(std::slice::from_mut(photo));
    photo.sender = "~Sam Sample".into();
    photo.sender_id = "demo:sam".into();
    photo.text = "[Photo]".into();
    std::sync::Arc::make_mut(&mut photo.content).body = Some(String::new());
    chats[5].messages = (0..10_000).map(|index| Message {
        id: next_id + index,
        sender: if index % 3 == 0 { "You" } else { "Timeline lab" }.into(),
        sender_id: if index % 3 == 0 { "#self" } else { "demo:timeline" }.into(),
        text: match index % 4 {
            0 => format!("Message {} · A short note.", index + 1),
            1 => format!("Message {} · A longer message to exercise wrapping as the window changes size. A readable conversation should stay comfortable on a small screen too.", index + 1),
            2 => format!("Message {}\nFirst line.\nSecond line.\nOne more for good measure.", index + 1),
            _ => format!("Message {} · Salut 👋 · مرحبا · こんにちは", index + 1),
        },
        time: format!("{:02}:{:02}", (index / 60) % 24, index % 60),
        date: Some(today - chrono::Days::new(((9999 - index) / 20) as u64)),
        outgoing: index % 3 == 0,
        local: false,
                    content: Default::default(),
    }).collect();
    let (preview, business) = content_samples();
    chats.push(Chat {
        id: 7, title: "Message cards".into(), initials: "MC".into(), tint: 0xa1c9df,
        is_group: false,
        unread: 0, draft: String::new(), visible_count: PAGE_SIZE,
        messages: vec![
            Message { id: 20_001, sender: "Sample contact".into(), sender_id: "demo:contact".into(), text: "https://example.invalid/community/news".into(), time: "10:28".into(), date: Some(today), outgoing:false, local:false, content: preview },
            Message { id: 20_002, sender: "Sample delivery".into(), sender_id: "demo:delivery".into(), text: "Your sample parcel is ready.\n\nUse code DEMO-42 at the collection point.\n\nExtend collection: https://example.invalid/delivery\n\nCopy code: DEMO-42".into(), time:"10:29".into(), date: Some(today), outgoing:false,local:false,content:business },
        ],
    });
    let mut dated = chats[6].clone();
    dated.id = 8;
    dated.title = "Dates and spacing".into();
    dated.initials = "DS".into();
    let template = dated.messages[1].clone();
    dated.messages = [14, 1, 0, 0, 0]
        .into_iter()
        .enumerate()
        .map(|(i, days)| {
            let mut message = template.clone();
            message.id = 21_001 + i as i32;
            message.date = Some(today - chrono::Days::new(days));
            message.time = format!("10:{:02}", 24 + i);
            if i == 0 || i >= 3 {
                message.content = Default::default();
                message.text = match i {
                    0 => "An earlier update, with its own date.",
                    3 => "Another message today, in the same run.",
                    _ => "Thanks! A new sender run has a little extra space.",
                }
                .into();
            }
            if i == 4 {
                message.outgoing = true;
                message.sender = "You".into();
                message.sender_id = "#self".into();
            }
            message
        })
        .collect();
    chats.push(dated);
    let mut photos = chats[7].clone();
    photos.id = 9;
    photos.title = "Received photos".into();
    photos.initials = "RP".into();
    photos.messages.truncate(3);
    for (i, message) in photos.messages.iter_mut().enumerate() {
        message.id = 22_002 + i as i32;
        message.date = Some(today);
    }
    fill_photos(&mut photos.messages);
    chats.push(photos);
    let mut audio = chats[8].clone();
    audio.id = 10;
    audio.title = "Received audio".into();
    audio.initials = "RA".into();
    for (i, m) in audio.messages.iter_mut().enumerate() {
        m.id = 23000 + i as i32;
        m.outgoing = i == 1;
    }
    fill_audio(&mut audio.messages);
    chats.push(audio);
    let mut documents = chats[9].clone();
    documents.id = 11;
    documents.title = "Received documents".into();
    documents.initials = "RD".into();
    for (i, m) in documents.messages.iter_mut().enumerate() {
        m.id = 24000 + i as i32;
        m.outgoing = false;
    }
    fill_documents(&mut documents.messages);
    chats.push(documents);
    let mut notices = chats[7].clone();
    notices.id = 12;
    notices.title = "Pin and unpin notices".into();
    notices.initials = "PN".into();
    notices.is_group = true;
    for (i, message) in notices.messages.iter_mut().enumerate() {
        message.id = 25000 + i as i32;
        message.date = Some(today);
        message.sender = if i == 2 { "You" } else { "Mira Sample" }.into();
        message.sender_id = if i == 2 { "#self" } else { "demo:mira" }.into();
        message.outgoing = i == 2;
        message.content = Default::default();
        let notice = match i {
            1 | 2 => Some(crate::message_content::Notice::MessagePinned),
            3 => Some(crate::message_content::Notice::MessageUnpinned),
            _ => None,
        };
        std::sync::Arc::make_mut(&mut message.content).notice = notice;
        message.text = notice
            .map_or("A sample message beside the system notices.", |n| {
                n.description()
            })
            .into();
    }
    chats.push(notices);
    let mut emoji = chats[0].clone();
    emoji.id = 13;
    emoji.title = "Color emoji".into();
    emoji.initials = "CE".into();
    emoji.messages = [
        "A sample note.",
        "😟",
        "👩🏽‍💻 🇷🇴 ❤️",
        "Mixed text 😟 · ❤️ · 👩🏽‍💻 · 🇷🇴 · #️⃣",
        "Text presentation: ❤\u{fe0e}",
        "😀😃😄😁",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, text)| Message {
        id: 26000 + i as i32,
        sender: "Sample contact".into(),
        sender_id: "demo:contact".into(),
        text: text.into(),
        time: format!("11:{:02}", 20 + i),
        date: Some(today),
        outgoing: false,
        local: false,
        content: Default::default(),
    })
    .collect();
    std::sync::Arc::make_mut(&mut emoji.messages[3].content).reaction_summary =
        "❤️ 12  👍🏽 2  😟".into();
    chats.push(emoji);
    for chat in &mut chats {
        for message in &mut chat.messages {
            std::sync::Arc::make_mut(&mut message.content).metadata =
                crate::message_actions::Metadata {
                    wire: format!("DEMO-{}", message.id),
                    stamp: chrono::Utc::now().timestamp() - 60,
                    delivery: if message.outgoing {
                        "accepted".into()
                    } else {
                        String::new()
                    },
                    ..Default::default()
                };
        }
    }
    let mut videos = chats[0].clone();
    videos.id = 14;
    videos.title = "Received videos".into();
    videos.initials = "RV".into();
    videos.is_group = false;
    videos.messages = (0..3)
        .map(|i| Message {
            id: 27001 + i,
            sender: "Sample contact".into(),
            sender_id: "demo:video".into(),
            text: if i == 1 {
                "[Video] A caption retained in full. 👋".into()
            } else {
                "[Video]".into()
            },
            time: format!("10:{:02}", 24 + i),
            date: Some(today),
            outgoing: i == 2,
            local: false,
            content: crate::message_content::Content {
                body: Some(if i == 1 {
                    "A caption retained in full. 👋".into()
                } else {
                    String::new()
                }),
                has_attachment: true,
                video: Some(crate::message_content::Video {
                    width: if i == 1 { 360 } else { 640 },
                    height: if i == 1 { 640 } else { 360 },
                    seconds: Some(5),
                    size: Some(1024),
                    gif: false,
                }),
                ..Default::default()
            }
            .into(),
        })
        .collect();
    chats.push(videos);
    let mut calls = chats[0].clone();
    calls.id = 15;
    calls.title = "Call history".into();
    calls.initials = "CH".into();
    calls.unread = 0;
    calls.messages = (0..6)
        .map(|i| {
            let mut m = calls.messages[0].clone();
            m.id = 43000 + i;
            m
        })
        .collect();
    fill_calls(&mut calls.messages);
    chats.push(calls);
    DemoState::new(chats)
}

pub fn fill_calls(messages: &mut [Message]) {
    use crate::message_content::{Call, CallOutcome, Content, Notice};
    for (i, m) in messages.iter_mut().enumerate() {
        let call = Call {
            video: Some(i % 3 == 1),
            outcome: match i % 3 {
                0 => CallOutcome::Missed,
                1 => CallOutcome::AnsweredElsewhere,
                _ => CallOutcome::Connected,
            },
            seconds: Some(125),
            revision: 0,
        };
        m.content = std::sync::Arc::new(Content {
            notice: Some(Notice::Call(call)),
            ..Default::default()
        });
        m.text = call.label().into();
        m.outgoing = i % 2 == 1;
        m.sender = if m.outgoing { "You" } else { "Sample contact" }.into();
        m.sender_id = if m.outgoing { "#self" } else { "demo:sample" }.into();
    }
}

pub fn fill_documents(messages: &mut [Message]) {
    use crate::message_content::{Content, Document};
    for (i, message) in messages.iter_mut().enumerate() {
        let document = Document {
            file_name: match i % 3 {
                0 => "Community_project_report_2026_signed.pdf",
                1 => "Travel_notes_こんにちは_2026.pdf",
                _ => "Planning_worksheet.xlsx",
            }
            .into(),
            mime_type: if i % 3 < 2 {
                "application/pdf"
            } else {
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            }
            .into(),
            pages: if i % 3 < 2 {
                Some(2 + i as u32 % 4)
            } else {
                None
            },
            size: Some(428000 + i as u64 % 200 * 1000),
            has_thumbnail: i % 3 < 2,
        };
        let body = if i % 3 == 2 {
            "The worksheet for our next meeting."
        } else {
            ""
        };
        message.text = document.transcript(body);
        message.content = std::sync::Arc::new(Content {
            body: Some(body.into()),
            document: Some(document),
            forwarded: i % 3 < 2,
            ..Default::default()
        });
    }
}

/// A real render of the synthetic PDF fixture, on a full portrait canvas.
/// Decoding this fixture happens in demo/benchmark preparation, off the UI loop.
pub fn document_thumbnail() -> image::RgbaImage {
    let strip = image::load_from_memory(include_bytes!("../tests/fixtures/preview-strip.png"))
        .expect("synthetic PDF render")
        .into_rgba8();
    let mut page = image::RgbaImage::from_pixel(768, 1024, image::Rgba([255; 4]));
    image::imageops::overlay(&mut page, &strip, 0, 0);
    page
}

pub fn message_image_samples(
    ids: impl IntoIterator<Item = i32>,
) -> crate::message_thumbnails::Snapshot {
    avatar_samples(ids)
        .iter()
        .map(|(id, thumbnail)| {
            let crate::avatars::Target::Chat(id) = id else {
                unreachable!()
            };
            (
                *id,
                std::sync::Arc::new(crate::photos::Pixels {
                    width: crate::avatars::EDGE,
                    height: crate::avatars::EDGE,
                    rgba: thumbnail.pixels.to_vec(),
                }),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>()
        .into()
}

/// Shared metadata for renderer/layout checks; never live URLs, codes or photos.
pub fn content_samples() -> (
    std::sync::Arc<crate::message_content::Content>,
    std::sync::Arc<crate::message_content::Content>,
) {
    use crate::message_content::{Content, Preview, action};
    let preview=Content { preview:Some(Preview {url:"https://example.invalid/community/news".into(),title:"Community news · a small shared project".into(),description:"A synthetic preview supplied with the message. It works without fetching the website.".into(),has_thumbnail:true}),..Default::default() };
    let business = Content {
        body: Some(
            "Your sample parcel is ready.\n\nUse code DEMO-42 at the collection point.".into(),
        ),
        actions: vec![
            action(
                Some("Extend collection"),
                Some("https://example.invalid/delivery"),
                "url",
            ),
            action(Some("Copy code"), Some("DEMO-42"), "copy"),
        ],
        ..Default::default()
    };
    (preview.into(), business.into())
}

/// A worst-case card timeline for explicit performance/layout checks.
pub fn fill_message_cards(messages: &mut [Message]) {
    let (preview, business) = content_samples();
    for (i, message) in messages.iter_mut().enumerate() {
        if i % 2 == 0 {
            message.text = "https://example.invalid/community/news".into();
            message.content = preview.clone();
        } else {
            message.content = business.clone();
        }
    }
}

/// Synthetic photo pixels only. Call on a worker, never from row layout.
pub fn photo_pixels(index: usize, edge: u32) -> crate::photos::Pixels {
    let (w, h) = match index % 3 {
        0 => (edge, edge * 3 / 4),
        1 => (edge / 2, edge),
        _ => (edge, edge / 4),
    };
    let image = image::RgbaImage::from_fn(w, h, |x, y| {
        if (x as i64 - w as i64 * 3 / 4).pow(2) + (y as i64 - h as i64 / 4).pow(2)
            < (h as i64 / 7).pow(2)
        {
            image::Rgba([255, 216, 117, 255])
        } else if y > h * 3 / 4 - x * h / (w * 3) {
            image::Rgba([31, 115 + (x % 45) as u8, 91, 255])
        } else {
            image::Rgba([65 + (x * 80 / w) as u8, 136 + (y * 60 / h) as u8, 196, 255])
        }
    });
    crate::photos::Pixels {
        width: w,
        height: h,
        rgba: image.into_raw(),
    }
}

pub fn fill_photos(messages: &mut [Message]) {
    for (i, message) in messages.iter_mut().enumerate() {
        let (width, height) = match message.id.unsigned_abs() % 3 {
            0 => (1600, 1200),
            1 => (900, 1800),
            _ => (2000, 500),
        };
        let body = match i%4 {
            0 => "A synthetic landscape · Salut 👋 · こんにちは".into(),
            1 => String::new(),
            2 => "A wide photo keeps its proportions.".into(),
            _ => "A longer caption, preserved in full.\n\nSecond paragraph with Unicode: مرحبا · 你好 · café.".into(),
        };
        message.text = format!("[Photo] {body}");
        message.content = std::sync::Arc::new(crate::message_content::Content {
            body: Some(body),
            photo: Some(crate::message_content::Photo {
                width,
                height,
                size: Some(123456),
            }),
            ..Default::default()
        });
    }
}

/// Synthetic silent WAV. Tests also exercise an Opus/Ogg encoding of this input.
pub fn audio_bytes() -> Vec<u8> {
    let data_len = 8000u32 * 2 * 4;
    let mut b = Vec::with_capacity(data_len as usize + 44);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&8000u32.to_le_bytes());
    b.extend_from_slice(&16000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    b.resize(data_len as usize + 44, 0);
    b
}
pub fn fill_audio(messages: &mut [Message]) {
    for (i, message) in messages.iter_mut().enumerate() {
        message.text = "[Voice message]".into();
        message.content = std::sync::Arc::new(crate::message_content::Content {
            body: Some(String::new()),
            audio: Some(crate::message_content::Audio {
                seconds: Some(4),
                voice: i % 2 == 0,
                size: Some(64044),
                waveform: (0..64)
                    .map(|v| ((v * 17 + i * 11) % 90 + 10) as u8)
                    .collect(),
            }),
            ..Default::default()
        });
    }
}
