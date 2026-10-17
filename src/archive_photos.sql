-- Media and derived previews stay in the same encrypted account archive.
CREATE TABLE message_photos(
    message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    original BLOB NOT NULL DEFAULT X'',
    preview BLOB NOT NULL DEFAULT X'',
    state INTEGER NOT NULL DEFAULT 0,
    revision INTEGER NOT NULL DEFAULT 0
);
-- Mutations must not leave a downloaded copy behind, even during a download.
CREATE TRIGGER discard_changed_photo AFTER UPDATE OF content,raw,thumbnail,deleted,visible ON messages
WHEN OLD.content<>NEW.content OR OLD.raw<>NEW.raw OR OLD.thumbnail<>NEW.thumbnail OR NEW.deleted=1 OR NEW.visible=0
BEGIN
    DELETE FROM message_photos WHERE message_id=NEW.id;
END;
