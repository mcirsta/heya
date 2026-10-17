-- Identical compressed replies may answer different requests. Keep every
-- request/chunk association instead of overwriting a single request ID.
CREATE TABLE history_requests (
    request_id TEXT NOT NULL,
    chunk_id TEXT NOT NULL REFERENCES history_chunks(id),
    PRIMARY KEY(request_id,chunk_id)
);
CREATE TABLE history_pages (
    chunk_id TEXT NOT NULL REFERENCES history_chunks(id),
    chat_id INTEGER NOT NULL REFERENCES chats(id),
    more INTEGER,
    PRIMARY KEY(chunk_id,chat_id)
);
CREATE INDEX history_pages_chat ON history_pages(chat_id);

-- Avoid decrypting/scanning non-expiring messages on every view/ingest. The
-- visible index also covers the sidebar's newest-message lookup.
CREATE INDEX IF NOT EXISTS messages_expiring ON messages(deleted,expires,wire_id)
    WHERE expires IS NOT NULL;
CREATE INDEX IF NOT EXISTS messages_visible_timeline ON messages(chat_id,stamp,id)
    WHERE visible=1;
