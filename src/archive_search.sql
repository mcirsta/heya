-- Search lives in the account archive and follows its encryption setting.
-- Existing history is backfilled in short transactions by the messaging worker.
CREATE VIRTUAL TABLE message_words USING fts5(
    body, content='messages', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);
INSERT INTO message_words(message_words,rank) VALUES('secure-delete',1);
CREATE TABLE search_progress(id INTEGER PRIMARY KEY CHECK(id=1), cursor INTEGER NOT NULL, end_id INTEGER NOT NULL);
INSERT INTO search_progress SELECT 1,0,coalesce(max(id),0) FROM messages;

CREATE TRIGGER message_words_insert AFTER INSERT ON messages BEGIN
    INSERT INTO message_words(rowid,body) VALUES(new.id,new.body);
END;
-- Rows not reached by the backfill have no index entries to delete yet.
CREATE TRIGGER message_words_delete AFTER DELETE ON messages
WHEN old.id<=(SELECT cursor FROM search_progress) OR old.id>(SELECT end_id FROM search_progress)
BEGIN
    INSERT INTO message_words(message_words,rowid,body) VALUES('delete',old.id,old.body);
END;
CREATE TRIGGER message_words_update AFTER UPDATE OF body ON messages
WHEN old.body<>new.body AND
    (old.id<=(SELECT cursor FROM search_progress) OR old.id>(SELECT end_id FROM search_progress))
BEGIN
    INSERT INTO message_words(message_words,rowid,body) VALUES('delete',old.id,old.body);
    INSERT INTO message_words(rowid,body) VALUES(new.id,new.body);
END;
