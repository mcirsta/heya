-- An immutable, bounded image strip, never the original document. AUTOINCREMENT
-- prevents a deleted/recreated cache entry reusing an in-memory image revision.
CREATE TABLE document_previews(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL UNIQUE REFERENCES messages(id) ON DELETE CASCADE,
    preview BLOB NOT NULL DEFAULT X'',
    retry_at INTEGER NOT NULL DEFAULT 0
);
CREATE TRIGGER discard_changed_document_preview AFTER UPDATE OF content,raw,thumbnail,deleted,visible ON messages
WHEN OLD.content<>NEW.content OR OLD.raw<>NEW.raw OR OLD.thumbnail<>NEW.thumbnail OR NEW.deleted=1 OR NEW.visible=0
BEGIN
    DELETE FROM document_previews WHERE message_id=NEW.id;
END;
