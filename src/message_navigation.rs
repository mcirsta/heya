//! Local history search and identity-based navigation; no UI or SDK types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub id: i32,
    pub text: String,
    pub time: String,
}
#[derive(Clone, Default)]
pub struct Search {
    pub chat: i32,
    pub request: i32,
    pub query: String,
    pub hits: Vec<Hit>,
    pub more: bool,
}
#[derive(Clone, Copy, Default)]
pub struct Jump {
    pub request: i32,
    pub message: i32,
    pub browsing: bool,
    pub returning: bool,
}

pub enum Command {
    Search {
        chat: i32,
        query: String,
        request: i32,
        revision: i32,
    },
    Navigate {
        chat: i32,
        message: i32,
        quote: bool,
        request: i32,
        revision: i32,
    },
}
