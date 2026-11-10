//! Bounded vCard display data. Original cards stay in the message envelope.
use serde::{Deserialize, Serialize};

pub const MAX_CARDS: usize = 257;
pub const MAX_BYTES: usize = 5_000_000;
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub name: String,
    pub phones: Vec<Phone>,
    pub fields: Vec<Field>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phone {
    pub number: String,
    /// Sender-declared WhatsApp account; displaying it never adds a contact.
    pub account: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub label: String,
    pub value: String,
}
impl Contact {
    pub fn account(&self) -> &str {
        self.phones
            .iter()
            .find(|p| !p.account.is_empty())
            .map_or("", |p| p.account.as_str())
    }
    pub fn details(&self) -> String {
        self.phones
            .iter()
            .map(|p| p.number.clone())
            .chain(
                self.fields
                    .iter()
                    .map(|f| format!("{}: {}", f.label, f.value)),
            )
            .collect::<Vec<_>>()
            .join("\n")
    }
}
pub fn summary(contacts: &[Contact]) -> String {
    match contacts {
        [] => "Contact".into(),
        [one] => format!("Contact: {}", one.name),
        _ => format!("{} contacts", contacts.len()),
    }
}
fn clean(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(if matches!(next, 'n' | 'N') {
                    '\n'
                } else {
                    next
                });
            }
        } else if !c.is_control() || c == '\n' {
            out.push(c);
        }
        if out.len() > 16 * 1024 {
            break;
        }
    }
    out.trim().to_owned()
}
fn parts(value: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (i, c) in value.char_indices() {
        if c == ';' && !escaped {
            result.push(clean(&value[start..i]));
            start = i + 1;
        }
        escaped = c == '\\' && !escaped;
    }
    result.push(clean(&value[start..]));
    result
}
fn quoted(value: &str, latin: bool) -> String {
    let mut out = Vec::new();
    let mut bytes = value.as_bytes().iter().copied();
    while let Some(b) = bytes.next() {
        if b == b'=' {
            let pair = [
                bytes.next().unwrap_or_default(),
                bytes.next().unwrap_or_default(),
            ];
            if let Ok(s) = std::str::from_utf8(&pair)
                && let Ok(n) = u8::from_str_radix(s, 16)
            {
                out.push(n);
            }
        } else {
            out.push(b);
        }
    }
    if latin {
        out.into_iter().map(char::from).collect()
    } else {
        String::from_utf8_lossy(&out).into_owned()
    }
}
pub fn parse(display: &str, raw: &str) -> Contact {
    let mut card = Contact {
        // Protocol display names are plain text; only vCard values are escaped.
        name: display
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .take(4096)
            .collect::<String>()
            .trim()
            .to_owned(),
        ..Default::default()
    };
    if raw.len() > MAX_BYTES {
        if card.name.is_empty() {
            card.name = "Contact".into();
        }
        return card;
    }
    let mut lines: Vec<String> = Vec::new();
    for line in raw.lines().take(4096) {
        let line = line.trim_end_matches('\r');
        if let Some(last) = lines.last_mut() {
            if last.ends_with('=')
                && last
                    .to_ascii_uppercase()
                    .contains("ENCODING=QUOTED-PRINTABLE")
            {
                last.pop();
                if last.len() < 32 * 1024 {
                    last.push_str(line);
                }
                continue;
            }
            if line.starts_with([' ', '\t']) {
                if last.len() < 32 * 1024 {
                    last.push_str(&line[1..]);
                }
                continue;
            }
        }
        lines.push(line.to_owned());
    }
    if !lines
        .first()
        .is_some_and(|l| l.eq_ignore_ascii_case("BEGIN:VCARD"))
        || !lines
            .last()
            .is_some_and(|l| l.eq_ignore_ascii_case("END:VCARD"))
    {
        if card.name.is_empty() {
            card.name = "Contact".into();
        }
        return card;
    }
    for line in lines.iter().take(100) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.to_ascii_uppercase();
        let property = key
            .split(';')
            .next()
            .unwrap_or("")
            .rsplit('.')
            .next()
            .unwrap_or("");
        let value = if key.contains("ENCODING=QUOTED-PRINTABLE") {
            quoted(value, key.contains("CHARSET=ISO-8859-1"))
        } else {
            value.into()
        };
        let decoded = clean(&value);
        match property {
            "FN" if display.trim().is_empty() => card.name = decoded,
            "N" if card.name.is_empty() => {
                let p = parts(&value);
                card.name = p
                    .iter()
                    .skip(3)
                    .chain(p.iter().skip(1).take(2))
                    .chain(p.first())
                    .filter(|p| !p.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            "TEL" if card.phones.len() < 16 => {
                let account = key
                    .split(';')
                    .find_map(|p| p.strip_prefix("WAID="))
                    .and_then(crate::domain::phone_number)
                    .unwrap_or_default();
                let number = decoded.strip_prefix("tel:").unwrap_or(&decoded).to_owned();
                if !number.is_empty()
                    && !card
                        .phones
                        .iter()
                        .any(|p| p.number == number && p.account == account)
                {
                    card.phones.push(Phone { number, account });
                }
            }
            "ORG" | "TITLE" | "EMAIL" | "ADR" | "URL" | "BDAY" | "NOTE"
                if card.fields.len() < 32 =>
            {
                let label = match property {
                    "ORG" => "Organization",
                    "TITLE" => "Title",
                    "EMAIL" => "Email",
                    "ADR" => "Address",
                    "URL" => "Website",
                    "BDAY" => "Birthday",
                    _ => "Notes",
                };
                let value = if matches!(property, "ADR" | "ORG") {
                    parts(&value)
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    decoded
                };
                if !value.is_empty() {
                    card.fields.push(Field {
                        label: label.into(),
                        value,
                    });
                }
            }
            _ => {}
        }
    }
    if card.name.is_empty() {
        card.name = card
            .phones
            .first()
            .map_or("Contact", |p| p.number.as_str())
            .into();
    }
    card
}
