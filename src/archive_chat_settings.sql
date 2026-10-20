-- Independent of listed chats: app-state metadata must not create phantom rows.
CREATE TABLE chat_settings (
    jid TEXT NOT NULL,
    kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND 2),
    value INTEGER NOT NULL,
    stamp INTEGER,
    PRIMARY KEY(jid,kind)
);
