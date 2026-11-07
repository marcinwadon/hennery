-- List/SSE metadata must not scan a session's transcript for each refresh.
CREATE INDEX events_work_last_message ON events(owner_id,session_id,event_id DESC,ts)
WHERE applied=1 AND kind='acp_update'
  AND json_extract(body,'$.payload.update.sessionUpdate')='agent_message_chunk'
  AND json_extract(body,'$.payload.update.content.type')='text'
  AND length(json_extract(body,'$.payload.update.content.text'))>0;

CREATE INDEX events_work_last_activity ON events(owner_id,session_id,event_id DESC,ts)
WHERE applied=1 AND kind IN ('acp_update','turn_started','turn_ended','pending_opened','pending_resolved',
    'pending_cancelled','start_failed','session_started','session_parked','session_closed');
