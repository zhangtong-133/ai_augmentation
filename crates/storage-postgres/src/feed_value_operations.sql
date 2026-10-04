WITH evidence AS (
 SELECT v.id,v.status,v.created_ms,v.expires_ms,v.approved_ms,v.dispatch_deadline_ms,v.sent_ms,
        COALESCE(a.events,ARRAY[]::text[]) AS events,
        COALESCE(a.bad_time,false) AS bad_audit_time,
        COALESCE(a.sending_count,0) AS sending_count,a.sending_ms
 FROM feed_value_reviews v
 LEFT JOIN LATERAL (
  SELECT array_agg(event ORDER BY sequence) AS events,
         bool_or(at_ms<v.created_ms OR at_ms>$2) AS bad_time,
         count(*) FILTER(WHERE event='sending') AS sending_count,
         max(at_ms) FILTER(WHERE event='sending') AS sending_ms
  FROM feed_value_audit WHERE user_id=v.user_id AND request_id=v.id
 ) a ON true
 WHERE v.user_id=$1
), audit AS (
 SELECT *,array_remove(ARRAY[
  CASE WHEN created_ms>$2 OR approved_ms>$2 OR sent_ms>$2 THEN 'future_timestamp' END,
  CASE WHEN events[1] IS DISTINCT FROM 'draft' OR events[cardinality(events)] IS DISTINCT FROM status
    OR (approved_ms IS NOT NULL AND NOT ('authorized'=ANY(events)))
    OR (dispatch_deadline_ms IS NOT NULL AND NOT ('running'=ANY(events))) THEN 'state_audit_mismatch' END,
  CASE WHEN sending_count<>CASE WHEN sent_ms IS NULL THEN 0 ELSE 1 END
    OR sending_ms IS DISTINCT FROM sent_ms THEN 'sending_audit_mismatch' END,
  CASE WHEN bad_audit_time THEN 'invalid_audit_time' END
 ],NULL) AS issues FROM evidence
)
