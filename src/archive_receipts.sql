-- 0 handled/suppressed, 1 queued, 2 new incoming message not yet observed here.
-- Only a visible read promotes 2 to 1. Private badge updates cannot consume it.
CREATE TABLE outgoing_receipts(
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    kind INTEGER NOT NULL CHECK(kind IN (0,1)),
    state INTEGER NOT NULL DEFAULT 1 CHECK(state IN (0,1,2)),
    PRIMARY KEY(message_id,kind)
);
CREATE INDEX outgoing_receipts_state ON outgoing_receipts(message_id,kind) WHERE state=1;
CREATE INDEX outgoing_receipts_unobserved ON outgoing_receipts(message_id) WHERE state=2;
