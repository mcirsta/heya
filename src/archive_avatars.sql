-- Only normalized thumbnails; source URLs and original files are never retained.
CREATE TABLE avatars (
    jid TEXT PRIMARY KEY,
    generation INTEGER NOT NULL DEFAULT 0,
    picture_id TEXT,
    pixels BLOB CHECK(pixels IS NULL OR length(pixels)=65536),
    retry_at INTEGER NOT NULL DEFAULT 0,
    event_stamp INTEGER NOT NULL DEFAULT 0
);
