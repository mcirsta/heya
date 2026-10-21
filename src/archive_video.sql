-- Originals remain in SQLCipher; a player receives bytes over an anonymous pipe.
CREATE TABLE IF NOT EXISTS message_videos(
    message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    original BLOB NOT NULL
);
CREATE TRIGGER IF NOT EXISTS discard_changed_video AFTER UPDATE OF content,raw,deleted,visible ON messages
WHEN OLD.content<>NEW.content OR OLD.raw<>NEW.raw OR NEW.deleted=1 OR NEW.visible=0
BEGIN
    DELETE FROM message_videos WHERE message_id=NEW.id;
END;
