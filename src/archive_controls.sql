-- Control operations use invisible rows in the existing durable outbox.
-- Wire keys survive PN/LID merges that replace local row IDs.
CREATE TABLE IF NOT EXISTS message_controls(
    event_wire TEXT PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN ('edit','revoke','hide')),
    target TEXT NOT NULL, sender TEXT NOT NULL, outgoing INTEGER NOT NULL,
    stamp INTEGER NOT NULL, text TEXT NOT NULL, version INTEGER NOT NULL,
    applied INTEGER NOT NULL DEFAULT 0, retry_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS controls_target ON message_controls(target,sender,version);
CREATE TABLE IF NOT EXISTS message_hides(
    chat_id INTEGER NOT NULL REFERENCES chats(id), sender TEXT NOT NULL, wire_id TEXT NOT NULL,
    PRIMARY KEY(chat_id,sender,wire_id)
);
CREATE TABLE IF NOT EXISTS message_receipts(
    chat_id INTEGER NOT NULL REFERENCES chats(id), wire_id TEXT NOT NULL, recipient TEXT NOT NULL,
    delivered INTEGER, read INTEGER, played INTEGER,
    PRIMARY KEY(chat_id,wire_id,recipient)
);
