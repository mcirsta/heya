-- Expand the existing durable controls without changing their stored identities.
ALTER TABLE message_controls RENAME TO old_message_controls;
CREATE TABLE message_controls(
    event_wire TEXT PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN ('edit','revoke','hide','star','unstar','pin','unpin')),
    target TEXT NOT NULL, sender TEXT NOT NULL, outgoing INTEGER NOT NULL,
    stamp INTEGER NOT NULL, text TEXT NOT NULL, version INTEGER NOT NULL,
    applied INTEGER NOT NULL DEFAULT 0, retry_at INTEGER NOT NULL DEFAULT 0
);
INSERT INTO message_controls SELECT * FROM old_message_controls;
DROP TABLE old_message_controls;
CREATE INDEX controls_target ON message_controls(target,sender,version);
CREATE TABLE IF NOT EXISTS message_marks(
    chat_id INTEGER NOT NULL REFERENCES chats(id), sender TEXT NOT NULL, wire_id TEXT NOT NULL,
    starred INTEGER NOT NULL DEFAULT 0, star_stamp INTEGER,
    pinned_until INTEGER NOT NULL DEFAULT 0, pin_stamp INTEGER,
    PRIMARY KEY(chat_id,sender,wire_id)
);
