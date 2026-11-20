-- NULL preserves ordinary delivery for pre-upgrade operations.
ALTER TABLE external_operations ADD COLUMN steering_turn TEXT;
ALTER TABLE external_operations ADD COLUMN steering_deadline INTEGER;
ALTER TABLE external_operations ADD COLUMN steering_failure TEXT;
CREATE INDEX external_steering_request ON external_operations(owner_id,session_id,request_id) WHERE steering_turn IS NOT NULL;
CREATE INDEX external_steering_deadline ON external_operations(owner_id,steering_deadline) WHERE steering_deadline IS NOT NULL;

-- A turn's confirmed admission releases the FIFO for later active-turn input.
CREATE VIEW external_prompt_heads AS
SELECT o.owner_id,o.id FROM external_operations o
WHERE o.tool='session_prompt' AND o.state='queued' AND NOT EXISTS(
 SELECT 1 FROM external_operations p WHERE p.owner_id=o.owner_id AND p.session_id=o.session_id
 AND p.tool='session_prompt' AND p.position<o.position AND p.state IN ('queued','running','uncertain')
 AND COALESCE(json_extract(p.result,'$.admission'),'')<>'confirmed'
 AND NOT EXISTS(SELECT 1 FROM turns t WHERE t.owner_id=p.owner_id AND t.session_id=p.session_id
 AND t.turn_id=p.turn_id AND t.state IN ('started','ended')));

-- Public reads can precede the periodic ledger reconciliation. Only an
-- admitted steer can adopt its target's outcome. Apply this view before
-- pagination so a completed operation cannot take an active queue slot.
CREATE VIEW external_steering_receipts AS
SELECT o.owner_id,o.id,o.steering_turn AS turn_id,
 CASE WHEN o.state IN ('failed','cancelled') THEN o.state
      WHEN t.state='ended' THEN 'completed'
      WHEN t.state='not_delivered' THEN 'not_delivered'
      WHEN o.state='uncertain' THEN 'delivery_unknown'
      ELSE o.state END AS state,
 CASE WHEN o.state NOT IN ('failed','cancelled') AND t.state='ended' THEN t.outcome END AS outcome
FROM external_operations o LEFT JOIN turns t
 ON t.owner_id=o.owner_id AND t.session_id=o.session_id AND t.turn_id=o.steering_turn
 AND json_extract(o.result,'$.admission')='confirmed'
WHERE o.tool='session_prompt' AND o.steering_turn IS NOT NULL
 AND json_extract(o.result,'$.delivery')='steering';

CREATE TRIGGER external_steering_endpoint_end AFTER UPDATE OF close_requested,lifecycle,hat_id ON sessions
WHEN NEW.close_requested=1 OR NEW.lifecycle IN ('closed','failed','deleted') OR NEW.hat_id<>OLD.hat_id
BEGIN
 UPDATE external_operations SET steering_failure='endpoint_changed'
 WHERE owner_id=NEW.owner_id AND session_id=NEW.id AND steering_turn IS NOT NULL
 AND state IN ('running','uncertain') AND json_extract(result,'$.admission')='pending';
END;
CREATE TRIGGER external_steering_detached AFTER UPDATE OF lifecycle ON sessions
WHEN NEW.lifecycle IN ('parked','closed','failed') AND OLD.lifecycle<>NEW.lifecycle
BEGIN
 UPDATE external_operations SET state='uncertain',code='delivery_unknown',steering_deadline=NULL
 WHERE owner_id=NEW.owner_id AND session_id=NEW.id AND steering_deadline IS NOT NULL;
END;

-- Deletion also scrubs the snapshot from an operation with no owned turn.
CREATE TRIGGER external_steering_deleted BEFORE UPDATE OF lifecycle ON sessions WHEN NEW.lifecycle='deleted'
BEGIN
 UPDATE external_operations SET steering_turn=NULL,steering_failure=NULL,steering_deadline=NULL
 WHERE owner_id=NEW.owner_id AND session_id=NEW.id;
END;
