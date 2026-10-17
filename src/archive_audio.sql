-- Audio bytes remain inside SQLCipher. Never store playback file paths.
CREATE TABLE message_audio(
    message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    original BLOB NOT NULL
);
CREATE TRIGGER discard_changed_audio AFTER UPDATE OF content,raw,deleted,visible ON messages
WHEN OLD.content<>NEW.content OR OLD.raw<>NEW.raw OR NEW.deleted=1 OR NEW.visible=0
BEGIN
    DELETE FROM message_audio WHERE message_id=NEW.id;
END;
