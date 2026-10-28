-- Sessions migration 35: restore the latest result for children created before
-- child reviews were enabled. Schema migrations apply to every owner; runtime
-- reads and acknowledgements remain owner-scoped.
INSERT INTO session_reviews(owner_id,session_id,event_id,read_event_id)
SELECT s.owner_id,s.id,e.event_id,
  CASE WHEN s.seen_event_id>=e.event_id THEN e.event_id ELSE 0 END
FROM sessions s JOIN events e ON e.owner_id=s.owner_id AND e.session_id=s.id
  AND e.event_id=(SELECT MAX(last.event_id) FROM events last
    WHERE last.owner_id=s.owner_id AND last.session_id=s.id AND last.applied=1
    AND last.kind='turn_ended' AND json_extract(last.body,'$.outcome') IN ('completed','failed'))
WHERE s.delegated_child=1 AND s.lifecycle<>'deleted'
  AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id=s.owner_id AND h.hat_id=s.hat_id)
ON CONFLICT(owner_id,session_id) DO NOTHING;
