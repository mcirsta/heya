-- Album identity is independent of caption edits and media-cache fingerprints.
ALTER TABLE messages ADD COLUMN album TEXT;
ALTER TABLE messages ADD COLUMN album_index INTEGER;
CREATE INDEX messages_album ON messages(chat_id,sender,album,album_index IS NULL,album_index,stamp,id)
    WHERE album IS NOT NULL;
