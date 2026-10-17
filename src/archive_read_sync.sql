-- Separate from pending_reads, which holds inbound receipt IDs awaiting history.
-- Keep acknowledged positions to deduplicate repeated views of the same tail.
CREATE TABLE outgoing_reads(
    chat_id INTEGER PRIMARY KEY REFERENCES chats(id),
    stamp INTEGER NOT NULL,
    wire_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    pending INTEGER NOT NULL
);
CREATE INDEX outgoing_reads_pending ON outgoing_reads(chat_id) WHERE pending=1;
