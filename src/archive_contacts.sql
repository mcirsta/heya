-- Separate phonebook names from sender profile names and history chat titles.
-- NULL means no phonebook observation; '' is an explicit removal/empty name.
CREATE TABLE contacts(
    jid TEXT PRIMARY KEY,
    saved_name TEXT,
    saved_stamp INTEGER,
    profile_name TEXT NOT NULL DEFAULT ''
);
