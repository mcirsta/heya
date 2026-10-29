//! Compact timeline groups retain the underlying messages and their identities.
use crate::domain::Message;
use std::collections::HashMap;

/// A display identity. Inferred groups never become protocol associations.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Explicit(String),
    Inferred(i32),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub key: Key,
    pub index: Option<u32>,
}

pub const MAX_GAP_SECONDS: i64 = 60;

/// Small metadata only. `previous` is the preceding visible message, including
/// text, deleted placeholders and explicitly linked media that cannot join us.
pub struct Candidate {
    pub id: i32,
    pub previous: Option<i32>,
    pub stamp: i64,
    pub date: Option<chrono::NaiveDate>,
    pub sender: String,
    pub outgoing: bool,
    pub forwarded: bool,
    pub separate: bool,
}

pub fn separate(content: &crate::message_content::Content) -> bool {
    !content.body.as_deref().unwrap_or_default().is_empty()
        || content.quote.is_some()
        || !content.reaction_summary.is_empty()
        || !content.reaction_status.is_empty()
        || !content.actions.is_empty()
        || content.preview.is_some()
        || content.audio.is_some()
        || content.video.is_some()
        || content.document.is_some()
}

pub fn candidates(messages: &[Message]) -> Vec<Candidate> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.content.photo.is_some()
                && !matches!(
                    m.content.album.as_ref().map(|a| &a.key),
                    Some(Key::Explicit(_))
                )
        })
        .map(|(i, m)| Candidate {
            id: m.id,
            previous: i.checked_sub(1).map(|p| messages[p].id),
            stamp: m.content.metadata.stamp,
            date: m.date,
            sender: m.sender_id.clone(),
            outgoing: m.outgoing,
            forwarded: m.content.forwarded,
            separate: separate(&m.content),
        })
        .collect()
}

pub fn inferred(rows: &[Candidate]) -> Vec<Vec<i32>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    let mut previous: Option<&Candidate> = None;
    for row in rows {
        if !previous.is_some_and(|p| joins(p, row)) {
            if current.len() > 1 {
                groups.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
        current.push(row.id);
        previous = Some(row);
    }
    if current.len() > 1 {
        groups.push(current);
    }
    groups
}

pub fn joins(previous: &Candidate, next: &Candidate) -> bool {
    next.previous == Some(previous.id)
        && next.sender == previous.sender
        && next.outgoing == previous.outgoing
        && next.forwarded == previous.forwarded
        && next.date.is_some()
        && next.date == previous.date
        && previous.stamp > 0
        && next
            .stamp
            .checked_sub(previous.stamp)
            .is_some_and(|gap| (0..=MAX_GAP_SECONDS).contains(&gap))
        && !next.separate
}

pub fn annotate(messages: &mut [Message], groups: &[Vec<i32>]) {
    let members: HashMap<_, _> = groups
        .iter()
        .flat_map(|g| g.iter().map(|&id| (id, (g[0], g.len()))))
        .collect();
    for message in messages {
        if matches!(
            message.content.album.as_ref().map(|a| &a.key),
            Some(Key::Explicit(_))
        ) {
            continue;
        }
        let desired = members.get(&message.id).map(|&(head, _)| Group {
            key: Key::Inferred(head),
            index: None,
        });
        let count = members.get(&message.id).map_or(0, |(_, count)| *count);
        if message.content.album != desired || message.content.album_count != count {
            let content = std::sync::Arc::make_mut(&mut message.content);
            content.album = desired;
            content.album_count = count;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    /// Indices in album order, independent of arrival order.
    pub members: Vec<usize>,
    pub total: usize,
}
#[derive(Default)]
pub struct Layout {
    pub owners: Vec<usize>,
    pub grids: HashMap<usize, Grid>,
}
impl Layout {
    pub fn new(messages: &[Message], compact: bool) -> Self {
        if !compact
            || !messages
                .iter()
                .any(|m| m.content.album.is_some() && m.content.photo.is_some())
        {
            return Self::default();
        }
        let mut result = Self {
            owners: (0..messages.len()).collect(),
            grids: HashMap::new(),
        };
        let mut counts = HashMap::new();
        for m in messages {
            if let Some(a) = &m.content.album {
                *counts.entry((&m.sender_id, &a.key)).or_insert(0usize) += 1;
            }
        }
        let mut start = 0;
        while start < messages.len() {
            let first = &messages[start];
            let Some(album) = first
                .content
                .album
                .as_ref()
                .filter(|_| first.content.photo.is_some())
            else {
                start += 1;
                continue;
            };
            let mut end = start + 1;
            while let Some(next) = messages.get(end) {
                if next.content.photo.is_none()
                    || next.content.album.as_ref().is_none_or(|a| a.key != album.key)
                    || next.sender_id != first.sender_id || next.outgoing != first.outgoing
                    || next.date != first.date || next.content.forwarded != first.content.forwarded
                    // Preserve separately captioned/quoted/reacted messages in the chat.
                    || separate(&next.content)
                {
                    break;
                }
                end += 1;
            }
            if end > start + 1 {
                let mut members: Vec<_> = (start..end).collect();
                members.sort_by_key(|&i| {
                    let index = messages[i].content.album.as_ref().and_then(|a| a.index);
                    (
                        index.is_none(),
                        index,
                        messages[i].content.metadata.stamp,
                        messages[i].id,
                    )
                });
                // A full contiguous run can include cached members outside this
                // page. Split runs use their own count instead of double-counting.
                let total = if counts[&(&first.sender_id, &album.key)] == members.len() {
                    members
                        .iter()
                        .map(|&i| messages[i].content.album_count)
                        .max()
                        .unwrap_or(0)
                        .max(members.len())
                } else {
                    members.len()
                };
                result.owners[start..end].fill(start);
                result.grids.insert(start, Grid { members, total });
            }
            start = end;
        }
        result
    }
    pub fn owner(&self, index: usize) -> usize {
        self.owners.get(index).copied().unwrap_or(index)
    }
    pub fn hidden(&self, index: usize) -> bool {
        self.owner(index) != index
    }
    pub fn is_last(&self, index: usize, len: usize) -> bool {
        self.grids
            .get(&index)
            .map_or(index, |g| *g.members.iter().max().unwrap())
            + 1
            == len
    }
}

/// Shared geometry rules for native right-click hit testing. Gaps belong to
/// the nearest tile; all coordinates come from the actual Slint size probe.
pub fn tile_at(count: usize, width: f32, height: f32, x: f32, y: f32) -> Option<usize> {
    if x < 0. || y < 0. || x >= width || y >= height || count < 2 {
        return None;
    }
    Some(if count == 2 {
        usize::from(x >= width / 2.)
    } else if count == 3 {
        if x < width / 2. {
            0
        } else {
            1 + usize::from(y >= height / 2.)
        }
    } else {
        usize::from(y >= height / 2.) * 2 + usize::from(x >= width / 2.)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(count: usize) -> Vec<Message> {
        let mut demo = crate::fixtures::demo();
        demo.select(9);
        crate::fixtures::album_messages(&demo.conversation().messages[0], count)
    }
    fn unlinked(count: usize) -> Vec<Message> {
        let mut source = sample(count);
        for (i, m) in source.iter_mut().enumerate() {
            let content = std::sync::Arc::make_mut(&mut m.content);
            content.album = None;
            content.album_count = 0;
            content.metadata.stamp = 1_700_000_040 + i as i64;
            m.date = crate::message_time::local(content.metadata.stamp).map(|t| t.date_naive());
        }
        source
    }
    #[test]
    fn unlinked_photos_share_the_grid_path_without_fabricating_wire_links() {
        let mut source = unlinked(1000);
        let original = source.clone();
        let groups = inferred(&candidates(&source));
        assert_eq!(groups.len(), 1);
        annotate(&mut source, &groups);
        assert_eq!(
            source[0].content.album.as_ref().unwrap().key,
            Key::Inferred(44000)
        );
        let layout = Layout::new(&source, true);
        assert_eq!(layout.grids[&0].total, 1000);
        assert!(layout.owners.iter().all(|&owner| owner == 0));
        for (old, new) in original.iter().zip(&source) {
            assert_eq!(old.id, new.id);
            assert_eq!(old.transcript_line(), new.transcript_line());
            assert_eq!(
                serde_json::to_string(&*old.content).unwrap(),
                serde_json::to_string(&*new.content).unwrap()
            );
        }
        let content = source[0].content.clone();
        annotate(&mut source, &groups);
        assert!(std::sync::Arc::ptr_eq(&content, &source[0].content));
    }
    #[test]
    fn unlinked_boundaries_keep_authors_content_and_time_separate() {
        let original = unlinked(6);
        assert_eq!(inferred(&candidates(&original)).len(), 1);
        for variant in 0..9 {
            let mut source = original.clone();
            match variant {
                0 => source[2].sender_id = "other-person-same-display-name".into(),
                1 => source[2].outgoing = true,
                2 => std::sync::Arc::make_mut(&mut source[2].content).forwarded = false,
                3 => {
                    std::sync::Arc::make_mut(&mut source[2].content).body =
                        Some("Visible caption 👩🏽‍💻".into())
                }
                4 => {
                    std::sync::Arc::make_mut(&mut source[2].content).quote =
                        Some(crate::message_content::Quote {
                            sender: "Sample".into(),
                            text: "Visible quote".into(),
                        })
                }
                5 => {
                    std::sync::Arc::make_mut(&mut source[2].content).reaction_summary = "❤️".into()
                }
                6 => std::sync::Arc::make_mut(&mut source[2].content).photo = None,
                7 => source[2].date = None,
                _ => {
                    std::sync::Arc::make_mut(&mut source[2].content).album = Some(Group {
                        key: Key::Explicit("real-parent".into()),
                        index: Some(0),
                    })
                }
            }
            let groups = inferred(&candidates(&source));
            assert!(
                groups
                    .iter()
                    .all(|g| !(g.contains(&source[1].id) && g.contains(&source[2].id))),
                "boundary {variant}"
            );
        }
        let mut source = original;
        let stamp = source[1].content.metadata.stamp;
        std::sync::Arc::make_mut(&mut source[2].content)
            .metadata
            .stamp = stamp + MAX_GAP_SECONDS;
        assert!(joins(&candidates(&source)[1], &candidates(&source)[2]));
        std::sync::Arc::make_mut(&mut source[2].content)
            .metadata
            .stamp += 1;
        assert!(!joins(&candidates(&source)[1], &candidates(&source)[2]));
    }
    #[test]
    fn compact_grid_keeps_originals_counts_and_explicit_order() {
        let mut source = sample(38);
        source.swap(0, 3);
        let layout = Layout::new(&source, true);
        assert_eq!(layout.grids.len(), 1);
        assert_eq!(layout.grids[&0].total, 38);
        assert_eq!(
            layout.grids[&0]
                .members
                .iter()
                .take(4)
                .map(|&i| source[i].id)
                .collect::<Vec<_>>(),
            [44000, 44001, 44002, 44003]
        );
        assert!(layout.owners.iter().all(|&owner| owner == 0));
        assert!(layout.is_last(0, 38));
        assert_eq!(
            source.len(),
            38,
            "a grid does not erase messages or their actions"
        );
        let page = Layout::new(&source[34..], true);
        assert_eq!(
            page.grids[&0].total, 38,
            "counts include cached photos beyond the page"
        );
        let expanded = Layout::new(&source, false);
        assert!(expanded.grids.is_empty());
        assert!(
            expanded
                .owners
                .iter()
                .enumerate()
                .all(|(i, &owner)| i == owner)
        );
    }
    #[test]
    fn unrelated_photos_and_distinct_authors_never_join() {
        let mut source = sample(4);
        for m in &mut source {
            std::sync::Arc::make_mut(&mut m.content).album = None;
        }
        assert!(Layout::new(&source, true).grids.is_empty());
        let mut source = sample(4);
        source[1].sender_id = "another-person-with-the-same-name".into();
        let layout = Layout::new(&source, true);
        assert_eq!(layout.owner(1), 1);
        assert_eq!(layout.grids[&2].members, [2, 3]);
        assert!(!layout.grids.contains_key(&0));
    }
    #[test]
    fn captions_reactions_and_deleted_members_keep_separate_rows() {
        let mut source = sample(8);
        std::sync::Arc::make_mut(&mut source[4].content).body = Some("Preserved caption 👩🏽‍💻".into());
        let layout = Layout::new(&source, true);
        assert_eq!(layout.grids.len(), 2);
        assert_eq!(layout.grids[&0].total, 4);
        assert_eq!(layout.grids[&4].total, 4);
        assert_eq!(layout.owner(4), 4);
        std::sync::Arc::make_mut(&mut source[2].content).reaction_summary = "❤️ 2".into();
        assert_eq!(Layout::new(&source, true).owner(2), 2);
        std::sync::Arc::make_mut(&mut source[1].content).photo = None;
        assert!(!Layout::new(&source, true).hidden(1));
    }
}
