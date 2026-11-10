//! Explicitly selected name/number pairs; no phonebook or protocol ownership.
#[cfg(test)]
#[path = "contact_sending_tests.rs"]
mod tests;
pub const PAGE_SIZE: usize = 100;
pub const MAX_SELECTED: usize = crate::shared_contacts::MAX_CARDS;
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Choice {
    pub name: String,
    pub number: String,
}
impl Choice {
    pub fn valid(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.chars().count() <= 256
            && !self.name.chars().any(char::is_control)
            && crate::domain::phone_number(&self.number).as_deref() == Some(&self.number)
    }
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        self.name.to_lowercase().contains(&query)
            || (!query.is_empty()
                && query
                    .chars()
                    .all(|c| c.is_ascii_digit() || "+ ()-".contains(c))
                && self.number.contains(
                    &query
                        .chars()
                        .filter(char::is_ascii_digit)
                        .collect::<String>(),
                ))
    }
}
#[derive(Clone, Default)]
pub struct Page {
    pub choices: std::sync::Arc<[Choice]>,
    pub more: bool,
}
pub fn demo(query: &str) -> Page {
    Page {
        choices: [
            "Alex Sample",
            "Mîra Sample",
            "Sam Sample",
            "Sample Workshop",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, name)| Choice {
            name: name.into(),
            number: format!("1555000000{}", i + 1),
        })
        .filter(|c| c.matches(query))
        .collect::<Vec<_>>()
        .into(),
        more: false,
    }
}
