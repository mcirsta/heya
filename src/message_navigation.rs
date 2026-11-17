//! Local history search and identity-based navigation; no UI or SDK types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub id: i32,
    pub chat: i32,
    pub title: String,
    pub text: String,
    pub time: String,
}
#[derive(Clone, Default)]
pub struct Search {
    /// Zero searches all accessible chats; positive IDs keep the local search.
    pub chat: i32,
    pub request: i32,
    pub query: String,
    pub hits: Vec<Hit>,
    pub more: bool,
    pub indexing: bool,
    pub error: String,
}
#[derive(Clone, Copy, Default)]
pub struct Jump {
    pub request: i32,
    pub message: i32,
    pub browsing: bool,
    pub returning: bool,
}

pub enum Command {
    Open {
        chat: i32,
        message: i32,
        previous: i32,
        draft: String,
        draft_version: i32,
        request: i32,
        revision: i32,
    },
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

/// Tokenize input with the index's tokenizer, including Unicode and punctuation.
/// Quoting each token makes AND/OR, quotes and '*' ordinary input, never syntax.
pub(crate) fn word_query(db: &rusqlite::Connection, query: &str) -> rusqlite::Result<String> {
    db.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS temp.search_input USING fts5(body,tokenize='unicode61 remove_diacritics 2');
        CREATE VIRTUAL TABLE IF NOT EXISTS temp.search_terms USING fts5vocab(search_input,'row');
        DELETE FROM search_input;")?;
    db.execute("INSERT INTO search_input(body) VALUES(?1)", [query])?;
    let terms = db
        .prepare_cached("SELECT term FROM search_terms ORDER BY term")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(terms
        .into_iter()
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND "))
}
