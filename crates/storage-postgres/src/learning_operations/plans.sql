WITH audit AS (
 SELECT p.request_id,p.status,p.snapshot_revision,p.created_ms,
 array_remove(ARRAY[
   CASE WHEN p.created_ms>$2 THEN 'creation_in_future' END,
   CASE WHEN p.snapshot_revision>COALESCE((SELECT revision FROM learning_state WHERE user_id=$1),0) THEN 'snapshot_ahead_of_state' END,
   CASE WHEN (SELECT count(*) FROM learning_tasks t WHERE t.user_id=p.user_id AND t.request_id=p.request_id)>5
     OR EXISTS (SELECT 1 FROM learning_tasks t WHERE t.user_id=p.user_id AND t.request_id=p.request_id
       GROUP BY t.request_id HAVING min(t.ordinal)<>0 OR max(t.ordinal)<>count(*)-1)
     THEN 'invalid_task_ordinals' END,
   CASE WHEN EXISTS (SELECT 1 FROM learning_tasks t WHERE t.user_id=p.user_id AND t.request_id=p.request_id
     AND t.status<>CASE WHEN p.status='ready' THEN 'planned' ELSE p.status END) THEN 'task_status_mismatch' END,
   CASE WHEN p.status='ready' AND NOT EXISTS (SELECT 1 FROM learning_plan_sources s WHERE s.user_id=p.user_id AND s.request_id=p.request_id)
     THEN 'missing_plan_sources' END,
   CASE WHEN p.status='deleted' AND EXISTS (SELECT 1 FROM learning_plan_sources s WHERE s.user_id=p.user_id AND s.request_id=p.request_id)
     THEN 'deleted_plan_sources' END,
   CASE WHEN (SELECT count(*) FROM learning_plan_sources s WHERE s.user_id=p.user_id AND s.request_id=p.request_id)>100
     THEN 'plan_sources_limit_exceeded' END,
   CASE WHEN EXISTS (SELECT 1 FROM learning_results r JOIN learning_tasks t ON t.user_id=r.user_id AND t.id=r.task_id
     WHERE t.user_id=p.user_id AND t.request_id=p.request_id AND (p.status<>'ready' OR t.status<>'planned')) THEN 'result_on_inactive_task' END,
   CASE WHEN EXISTS (SELECT 1 FROM learning_results r JOIN learning_tasks t ON t.user_id=r.user_id AND t.id=r.task_id
     WHERE t.user_id=p.user_id AND t.request_id=p.request_id AND (r.recorded_ms<p.created_ms OR r.recorded_ms>$2)) THEN 'invalid_result_time' END
 ],NULL) AS issues
 FROM learning_plans p WHERE p.user_id=$1
)
