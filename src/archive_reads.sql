-- Read positions are independent of delivery receipts for our outgoing messages.
ALTER TABLE messages ADD COLUMN seen INTEGER NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN unread INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chats ADD COLUMN listed INTEGER NOT NULL DEFAULT 1;
ALTER TABLE chats ADD COLUMN read_stamp INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chats ADD COLUMN read_row INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chats ADD COLUMN unread_extra INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chats ADD COLUMN history_stamp INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chats ADD COLUMN history_count INTEGER NOT NULL DEFAULT -1;
ALTER TABLE chats ADD COLUMN marked_unread INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chats ADD COLUMN marked_stamp INTEGER;
CREATE TABLE pending_reads(
    chat_id INTEGER NOT NULL REFERENCES chats(id), wire_id TEXT NOT NULL,
    PRIMARY KEY(chat_id,wire_id)
);
CREATE INDEX chats_listed ON chats(id) WHERE listed=1;
CREATE INDEX messages_read_identity ON messages(chat_id,wire_id,outgoing);
CREATE INDEX messages_unread ON messages(chat_id,unread,stamp,id);
-- Preserve old counts until authoritative read state repairs them. The old
-- schema did not record which messages contributed, so seed the newest ones.
UPDATE messages SET unread=1 WHERE id IN (
    SELECT ranked.id FROM (
        SELECT id,chat_id,row_number() OVER(PARTITION BY chat_id ORDER BY stamp DESC,id DESC) AS n
        FROM messages WHERE outgoing=0 AND visible=1
    ) ranked JOIN chats c ON c.id=ranked.chat_id WHERE n<=c.unread
);
UPDATE chats SET unread_extra=max(0,unread-(SELECT count(*) FROM messages WHERE chat_id=chats.id AND unread=1));
