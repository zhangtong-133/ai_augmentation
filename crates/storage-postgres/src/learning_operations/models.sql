WITH evidence AS (
 SELECT a.request_id,a.plan_id,a.task_id,a.connection_id,a.connection_revision,a.status, a.connection_id IS NULL AS local,
        a.created_ms,a.expires_ms,a.approved_ms,a.dispatch_deadline_ms,a.sent_ms,
        a.status IN ('draft','authorized','running','succeeded') AS live,
        p.status AS plan_status,t.status AS task_status,t.request_id AS task_plan,
        r.outcome,e.deleted AS evidence_deleted,
        c.status AS connection_status,c.revision AS current_connection_revision,c.expires_ms AS connection_expires,
        COALESCE(events.names,ARRAY[]::text[]) AS events,
        COALESCE(events.bad_time,FALSE) AS bad_audit_time,
        events.sending_ms
 FROM learning_model_authorizations a
 LEFT JOIN learning_plans p ON p.user_id=a.user_id AND p.request_id=a.plan_id
 LEFT JOIN learning_tasks t ON t.user_id=a.user_id AND t.id=a.task_id
 LEFT JOIN learning_results r ON r.user_id=a.user_id AND r.task_id=a.task_id
 LEFT JOIN learning_evidence e ON e.user_id=a.user_id AND e.task_id=a.task_id
 LEFT JOIN subscription_connections c ON c.user_id=a.user_id AND c.id=a.connection_id
 LEFT JOIN LATERAL (
   SELECT array_agg(event) AS names,bool_or(at_ms<a.created_ms OR at_ms>$2) AS bad_time,
          max(at_ms) FILTER(WHERE event='sending') AS sending_ms
   FROM learning_model_authorization_audit WHERE user_id=a.user_id AND request_id=a.request_id
 ) events ON TRUE
 WHERE a.user_id=$1
), audit AS (
 SELECT *, array_remove(ARRAY[
   CASE WHEN created_ms>$2 OR approved_ms>$2 OR sent_ms>$2 THEN 'future_timestamp' END,
   CASE WHEN status='draft' AND approved_ms IS NOT NULL THEN 'draft_has_approval' END,
   CASE WHEN status IN ('authorized','running','succeeded') AND approved_ms IS NULL THEN 'missing_approval' END,
   CASE WHEN dispatch_deadline_ms IS NOT NULL AND (approved_ms IS NULL OR dispatch_deadline_ms<=approved_ms OR dispatch_deadline_ms>expires_ms) THEN 'invalid_dispatch_deadline' END,
   CASE WHEN status IN ('running','succeeded') AND dispatch_deadline_ms IS NULL THEN 'missing_dispatch_deadline' END,
   CASE WHEN sent_ms IS NOT NULL AND (dispatch_deadline_ms IS NULL OR approved_ms IS NULL OR sent_ms<approved_ms OR sent_ms>=dispatch_deadline_ms) THEN 'invalid_send_time' END,
   CASE WHEN status='succeeded' AND sent_ms IS NULL THEN 'success_without_send' END,
   CASE WHEN task_plan IS NOT NULL AND task_plan<>plan_id THEN 'task_plan_mismatch' END,
   CASE WHEN live AND (plan_status IS DISTINCT FROM 'ready' OR task_status IS DISTINCT FROM 'planned' OR outcome IS DISTINCT FROM 'completed' OR evidence_deleted IS DISTINCT FROM FALSE) THEN 'active_source_unavailable' END,
   CASE WHEN live AND NOT local AND (connection_status IS DISTINCT FROM 'active' OR current_connection_revision IS DISTINCT FROM connection_revision) THEN 'active_connection_unavailable' END,
   CASE WHEN NOT ('draft'=ANY(events)) OR NOT (status=ANY(events)) OR (approved_ms IS NOT NULL AND NOT ('authorized'=ANY(events))) OR (dispatch_deadline_ms IS NOT NULL AND NOT ('running'=ANY(events))) THEN 'missing_state_audit' END,
   CASE WHEN sending_ms IS DISTINCT FROM sent_ms THEN 'sending_audit_mismatch' END,
   CASE WHEN bad_audit_time THEN 'invalid_audit_time' END,
   CASE WHEN NOT (events <@ ARRAY['draft','authorized','running','sending','succeeded','unknown','cancelled','expired','invalidated']::text[]) THEN 'unknown_audit_event' END
 ],NULL) AS issues,
 array_remove(ARRAY[
   CASE WHEN status IN ('draft','authorized') AND expires_ms<=$2 THEN 'authorization_expired_pending_cleanup' END,
   CASE WHEN status='running' AND dispatch_deadline_ms<=$2 THEN 'dispatch_deadline_elapsed' END,
   CASE WHEN live AND connection_status='active' AND connection_expires<=$2 THEN 'connection_expired_pending_cleanup' END,
   CASE WHEN status='unknown' THEN 'outcome_unknown_no_retry' END
 ],NULL) AS warnings
 FROM evidence
)
