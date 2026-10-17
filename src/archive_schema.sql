-- Each archive file is dedicated to one account; identity is checked before ingestion.
CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE aliases(account INTEGER NOT NULL DEFAULT 1 CHECK(account=1), alias TEXT PRIMARY KEY, canonical TEXT NOT NULL);
CREATE TABLE chats(
    id INTEGER PRIMARY KEY AUTOINCREMENT, account INTEGER NOT NULL DEFAULT 1 CHECK(account=1),
    jid TEXT NOT NULL UNIQUE, name TEXT NOT NULL DEFAULT '', is_group INTEGER NOT NULL DEFAULT 0,
    read_only INTEGER NOT NULL DEFAULT 0, ephemeral INTEGER NOT NULL DEFAULT 0,
    unread INTEGER NOT NULL DEFAULT 0, draft TEXT NOT NULL DEFAULT ''
);
CREATE TABLE messages(
    id INTEGER PRIMARY KEY AUTOINCREMENT, account INTEGER NOT NULL DEFAULT 1 CHECK(account=1),
    chat_id INTEGER NOT NULL REFERENCES chats(id), sender TEXT NOT NULL, wire_id TEXT NOT NULL,
    source_chat TEXT NOT NULL, source_sender TEXT NOT NULL,
    stamp INTEGER NOT NULL, outgoing INTEGER NOT NULL, sender_name TEXT NOT NULL,
    body TEXT NOT NULL, raw BLOB NOT NULL, visible INTEGER NOT NULL, deleted INTEGER NOT NULL DEFAULT 0,
    expires INTEGER, delivery TEXT NOT NULL DEFAULT '', UNIQUE(chat_id,sender,wire_id)
);
CREATE INDEX messages_timeline ON messages(chat_id,stamp,id);
CREATE INDEX messages_outbox ON messages(delivery,id);
CREATE TABLE mutations(
    account INTEGER NOT NULL DEFAULT 1 CHECK(account=1), chat_id INTEGER NOT NULL REFERENCES chats(id),
    sender TEXT NOT NULL, wire_id TEXT NOT NULL, body TEXT, version INTEGER NOT NULL,
    deleted INTEGER NOT NULL, PRIMARY KEY(chat_id,sender,wire_id)
);
CREATE TABLE history_chunks(
    account INTEGER NOT NULL DEFAULT 1 CHECK(account=1), id TEXT PRIMARY KEY, data BLOB,
    progress INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL DEFAULT 'pending'
);

CREATE TABLE chat_redirects(id INTEGER PRIMARY KEY, target INTEGER NOT NULL REFERENCES chats(id));
