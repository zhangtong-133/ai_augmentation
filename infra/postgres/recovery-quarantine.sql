-- Executed only within the empty-target restore transaction, never as a migration.
-- A backup cannot prove which operations happened after its snapshot. Keep costs.
DELETE FROM sessions;
UPDATE mcp_credentials SET revoked_at=COALESCE(revoked_at,clock_timestamp());
UPDATE reply_configurations SET disabled_at=COALESCE(disabled_at,clock_timestamp());
UPDATE model_planning_configurations SET disabled_at=COALESCE(disabled_at,clock_timestamp());
UPDATE model_execution_configurations SET disabled_at=COALESCE(disabled_at,clock_timestamp());
UPDATE reply_money_reservations SET charged=reserved,settlement='retained',settled_at=clock_timestamp()
 WHERE settled_at IS NULL;

UPDATE document_index_jobs SET status='failed',error_code='recovery_paused',lease=NULL,lease_until=NULL
 WHERE status IN ('queued','running','retrying');
UPDATE conversation_replies SET status='cancelled',output=NULL WHERE status='queued';
UPDATE conversation_replies SET status='unknown',output=NULL WHERE status='dispatching';
UPDATE model_planning_requests SET status='cancelled',snapshot=NULL WHERE status IN ('draft','queued');
UPDATE model_planning_requests SET status='unknown',snapshot=NULL WHERE status='dispatching';
UPDATE model_execution_requests SET data=jsonb_set(
 jsonb_set(data,'{request}',(data->'request') || jsonb_build_object(
  'status',CASE WHEN data#>>'{request,status}'='draft' THEN 'cancelled' ELSE 'unknown' END,
  'searches',NULL,'evidence','[]'::jsonb,'answer',NULL)),
 '{steps}',COALESCE((SELECT jsonb_agg(step || jsonb_build_object('status',
   CASE step->>'status' WHEN 'reserved' THEN 'cancelled' WHEN 'dispatching' THEN 'unknown' ELSE step->>'status' END))
   FROM jsonb_array_elements(data->'steps') step),'[]'::jsonb))
 WHERE data#>>'{request,status}' IN ('draft','queued','running');
UPDATE model_execution_call_audit SET status=CASE status WHEN 'reserved' THEN 'cancelled' ELSE 'unknown' END
 WHERE status IN ('reserved','dispatching');
UPDATE agent_plans SET status='cancelled' WHERE status='draft';
UPDATE agent_plans SET status='unknown' WHERE status='running';
UPDATE agent_plan_steps SET status=CASE status WHEN 'pending' THEN 'cancelled' ELSE 'failed' END
 WHERE status IN ('pending','dispatching');
UPDATE tool_calls SET status='timed_out',finished_at=clock_timestamp() WHERE status='running';

UPDATE schedules SET status='cancelled',cancelled_ms=floor(extract(epoch FROM clock_timestamp())*1000)::bigint
 WHERE status IN ('draft','scheduled','running');
UPDATE feed_brief_schedules SET enabled=false,next_run_ms=NULL,revision=revision+1 WHERE enabled;
UPDATE feed_schedules SET status='cancelled' WHERE status IN ('draft','active');
UPDATE feed_collections SET status='cancelled',finished_ms=floor(extract(epoch FROM clock_timestamp())*1000)::bigint
 WHERE status='draft';
UPDATE feed_collections SET status='unknown',reason='unknown',finished_ms=floor(extract(epoch FROM clock_timestamp())*1000)::bigint
 WHERE status='running';
INSERT INTO feed_collection_audit(user_id,request_id,event,at_ms,reason,inserted,updated,unchanged)
 SELECT user_id,request_id,status,finished_ms,reason,inserted,updated,unchanged FROM feed_collections
 WHERE status IN ('cancelled','unknown') ON CONFLICT DO NOTHING;
UPDATE feed_value_reviews SET status='invalidated',snapshot=NULL,scores=NULL WHERE status IN ('draft','authorized');
UPDATE feed_value_reviews SET status='unknown',snapshot=NULL,scores=NULL WHERE status='running';
UPDATE learning_model_authorizations SET status='invalidated',advice=NULL WHERE status IN ('draft','authorized');
UPDATE learning_model_authorizations SET status='unknown',advice=NULL WHERE status='running';
