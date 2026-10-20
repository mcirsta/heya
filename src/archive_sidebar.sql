-- One indexed latest-message lookup per chat. Project only bounded display
-- strings; never load raw envelopes, thumbnails or entire Content values here.
SELECT c.id, c.jid,
    CASE WHEN n.saved_name IS NOT NULL THEN coalesce(nullif(n.saved_name,''),n.profile_name)
         ELSE coalesce(nullif(c.name,''),n.profile_name,'') END,
    c.unread, substr(c.draft,1,?1), coalesce(m.stamp,0), c.marked_unread,
    c.is_group, coalesce(m.outgoing,0),
    substr(coalesce(nullif(trim(s.saved_name),''),nullif(trim(s.profile_name),''),m.sender_name,''),1,?1),
    nullif(trim(s.saved_name),'') IS NOT NULL,
    coalesce(m.sender,''), substr(coalesce(m.body,''),1,?1),
    CASE WHEN m.deleted=0 AND json_valid(m.content) THEN
        CASE WHEN json_type(m.content,'$.photo')='object'
             THEN substr(coalesce(json_extract(m.content,'$.body'),''),1,?1) END
    END,
    CASE WHEN m.deleted=0 AND json_valid(m.content) THEN m.content -> '$.notice' END,
    coalesce(mute.value<0 OR mute.value>?2,0), coalesce(archive.value,0)
FROM chats c
LEFT JOIN chat_settings mute ON mute.jid=c.jid AND mute.kind=0
LEFT JOIN chat_settings archive ON archive.jid=c.jid AND archive.kind=1
LEFT JOIN chat_settings locked ON locked.jid=c.jid AND locked.kind=2
LEFT JOIN contacts n ON n.jid=c.jid
LEFT JOIN messages m ON m.id=(
    SELECT latest.id FROM messages latest
    WHERE latest.chat_id=c.id AND latest.visible=1
    ORDER BY latest.stamp DESC,latest.id DESC LIMIT 1
)
LEFT JOIN contacts s ON s.jid=m.sender
WHERE c.listed=1 AND coalesce(locked.value,0)=0
ORDER BY 6 DESC,c.id DESC
