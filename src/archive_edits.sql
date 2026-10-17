ALTER TABLE mutations ADD COLUMN raw BLOB NOT NULL DEFAULT X'';
ALTER TABLE messages ADD COLUMN edit_version INTEGER NOT NULL DEFAULT -1;
-- Captions and presentation metadata do not alter immutable media bytes. Raw
-- envelope changes still invalidate caches and reject stale in-flight results.
DROP TRIGGER discard_changed_photo;
CREATE TRIGGER discard_changed_photo AFTER UPDATE OF content,raw,thumbnail,deleted,visible ON messages
WHEN json_remove(coalesce(nullif(OLD.content,''),'{}'),'$.body','$.quote','$.forwarded','$.reaction')
  <> json_remove(coalesce(nullif(NEW.content,''),'{}'),'$.body','$.quote','$.forwarded','$.reaction')
  OR OLD.raw<>NEW.raw OR OLD.thumbnail<>NEW.thumbnail OR NEW.deleted=1 OR NEW.visible=0
BEGIN DELETE FROM message_photos WHERE message_id=NEW.id; END;
DROP TRIGGER discard_changed_audio;
CREATE TRIGGER discard_changed_audio AFTER UPDATE OF content,raw,deleted,visible ON messages
WHEN json_remove(coalesce(nullif(OLD.content,''),'{}'),'$.body','$.quote','$.forwarded','$.reaction')
  <> json_remove(coalesce(nullif(NEW.content,''),'{}'),'$.body','$.quote','$.forwarded','$.reaction')
  OR OLD.raw<>NEW.raw OR NEW.deleted=1 OR NEW.visible=0
BEGIN DELETE FROM message_audio WHERE message_id=NEW.id; END;
DROP TRIGGER discard_changed_document_preview;
CREATE TRIGGER discard_changed_document_preview AFTER UPDATE OF content,raw,thumbnail,deleted,visible ON messages
WHEN json_remove(coalesce(nullif(OLD.content,''),'{}'),'$.body','$.quote','$.forwarded','$.reaction')
  <> json_remove(coalesce(nullif(NEW.content,''),'{}'),'$.body','$.quote','$.forwarded','$.reaction')
  OR OLD.raw<>NEW.raw OR OLD.thumbnail<>NEW.thumbnail OR NEW.deleted=1 OR NEW.visible=0
BEGIN DELETE FROM document_previews WHERE message_id=NEW.id; END;
