-- The original encrypted update (or authenticated history hashes) lives in messages.
CREATE TABLE poll_events(
    event_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    target TEXT NOT NULL, author TEXT NOT NULL, version INTEGER NOT NULL,
    poll_id INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    selection TEXT,
    state INTEGER NOT NULL DEFAULT 0 CHECK(state BETWEEN 0 AND 2)
);
CREATE INDEX poll_events_target ON poll_events(target,event_id);
CREATE INDEX poll_events_parent ON poll_events(poll_id,version,event_id);
CREATE INDEX poll_events_unresolved ON poll_events(target,event_id)
    WHERE state=0 OR (poll_id IS NULL AND state=1);
