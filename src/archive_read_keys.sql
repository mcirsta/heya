-- A revoke changes WhatsApp's read-range key, not the original timeline position.
ALTER TABLE messages ADD COLUMN read_wire TEXT NOT NULL DEFAULT '';
CREATE INDEX messages_read_wire ON messages(chat_id,read_wire) WHERE read_wire<>'';
-- Keep the exact observed keys immutable while a read waits for the network.
ALTER TABLE outgoing_reads ADD COLUMN keys TEXT NOT NULL DEFAULT '';

-- Earlier versions discarded accepted protocol bodies. Recover only unambiguous
-- one-event/one-target pairs, using the accepted mutation's author and timestamp.
-- Never guess between multiple changes from the same author in the same second.
UPDATE messages AS m SET read_wire=(
    SELECT e.wire_id FROM mutations x JOIN messages e
      ON e.chat_id=x.chat_id AND e.sender=x.sender AND e.stamp=x.version/1000
    WHERE x.chat_id=m.chat_id AND x.sender=m.sender AND x.wire_id=m.wire_id
      AND x.deleted=1 AND x.version>100000000000
      AND e.visible=0 AND length(e.raw)=0
      AND (SELECT count(*) FROM messages candidate WHERE candidate.chat_id=e.chat_id
        AND candidate.sender=e.sender AND candidate.stamp=e.stamp
        AND candidate.visible=0 AND length(candidate.raw)=0)=1
      AND (SELECT count(*) FROM mutations target WHERE target.chat_id=x.chat_id
        AND target.sender=x.sender AND target.version=x.version)=1
) WHERE m.visible=1 AND m.deleted=1 AND m.body IN ('This message was deleted','[Message deleted]')
  AND (m.expires IS NULL OR m.expires>unixepoch()) AND EXISTS(
    SELECT 1 FROM mutations x WHERE x.chat_id=m.chat_id AND x.sender=m.sender
      AND x.wire_id=m.wire_id AND x.deleted=1 AND x.version>100000000000
      AND (SELECT count(*) FROM messages e WHERE e.chat_id=x.chat_id
        AND e.sender=x.sender AND e.stamp=x.version/1000 AND e.visible=0
        AND length(e.raw)=0)=1
      AND (SELECT count(*) FROM mutations target WHERE target.chat_id=x.chat_id
        AND target.sender=x.sender AND target.version=x.version)=1
);
