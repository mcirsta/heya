CREATE TABLE reply_drafts (
    chat_id INTEGER PRIMARY KEY REFERENCES chats(id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE
);
CREATE TABLE reactions (
    chat_id INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    target TEXT NOT NULL,
    actor TEXT NOT NULL,
    emoji TEXT NOT NULL,
    version INTEGER NOT NULL,
    event_wire TEXT NOT NULL,
    delivery TEXT NOT NULL DEFAULT '',
    PRIMARY KEY(chat_id,target,actor)
);
CREATE INDEX reactions_events ON reactions(chat_id,event_wire);
CREATE INDEX messages_action_wire ON messages(chat_id,wire_id);
-- Adding quote/forward display metadata does not alter the media payload.
-- All other content changes, mutations and envelope changes still purge it.
DROP TRIGGER discard_changed_photo;
CREATE TRIGGER discard_changed_photo AFTER UPDATE OF content,raw,thumbnail,deleted,visible ON messages
WHEN json_remove(coalesce(nullif(OLD.content,''),'{}'),'$.quote','$.forwarded','$.reaction')
  <> json_remove(coalesce(nullif(NEW.content,''),'{}'),'$.quote','$.forwarded','$.reaction')
  OR OLD.raw<>NEW.raw OR OLD.thumbnail<>NEW.thumbnail OR NEW.deleted=1 OR NEW.visible=0
BEGIN DELETE FROM message_photos WHERE message_id=NEW.id; END;
DROP TRIGGER discard_changed_audio;
CREATE TRIGGER discard_changed_audio AFTER UPDATE OF content,raw,deleted,visible ON messages
WHEN json_remove(coalesce(nullif(OLD.content,''),'{}'),'$.quote','$.forwarded','$.reaction')
  <> json_remove(coalesce(nullif(NEW.content,''),'{}'),'$.quote','$.forwarded','$.reaction')
  OR OLD.raw<>NEW.raw OR NEW.deleted=1 OR NEW.visible=0
BEGIN DELETE FROM message_audio WHERE message_id=NEW.id; END;
