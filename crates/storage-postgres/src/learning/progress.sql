WITH skills AS (
 SELECT s.id, (
   SELECT a.score FROM learning_assessments a
   WHERE a.user_id=s.user_id AND a.skill_id=s.id AND a.skill_revision=s.revision
     AND a.score IS NOT NULL AND a.assessed_ms<=$2
   ORDER BY a.assessed_ms DESC,a.id DESC LIMIT 1
 ) AS score
 FROM learning_skills s WHERE s.user_id=$1 AND s.enabled AND NOT s.deleted
), plans AS (
 SELECT request_id,snapshot_revision,NOT EXISTS(
   SELECT 1 FROM jsonb_array_elements(plan->'evaluations') e
   WHERE e->>'assessment_id' IS NOT NULL AND NOT EXISTS(
     SELECT 1 FROM learning_assessments a WHERE a.user_id=$1 AND a.id::text=e->>'assessment_id' AND a.score IS NOT NULL
   )
 ) AS sources_available FROM learning_plans WHERE user_id=$1 AND status='ready'
), tasks AS (
 SELECT t.id,r.outcome,r.actual_minutes,r.recorded_ms,p.sources_available
 FROM learning_tasks t JOIN plans p ON p.request_id=t.request_id
 LEFT JOIN learning_results r ON r.user_id=t.user_id AND r.task_id=t.id
 WHERE t.user_id=$1 AND t.status='planned'
)
SELECT
 (SELECT count(*) FROM skills) AS enabled_skills,
 (SELECT count(*) FROM skills WHERE score IS NOT NULL) AS assessed_skills,
 (SELECT count(*) FROM skills WHERE score>=$5) AS target_reached_skills,
 (SELECT count(*) FROM plans) AS ready_plans,
 (SELECT count(*) FROM plans WHERE snapshot_revision<$6) AS historical_plans,
 count(*) FILTER(WHERE outcome IS NULL AND sources_available) AS pending_tasks,
 count(*) FILTER(WHERE outcome='completed') AS completed_tasks,
 count(*) FILTER(WHERE outcome='cancelled') AS cancelled_tasks,
 count(*) FILTER(WHERE outcome='completed' AND recorded_ms>=$3 AND recorded_ms<$4 AND recorded_ms<=$2) AS completed_today,
 count(*) FILTER(WHERE outcome='cancelled' AND recorded_ms>=$3 AND recorded_ms<$4 AND recorded_ms<=$2) AS cancelled_today,
 COALESCE(sum(actual_minutes) FILTER(WHERE outcome='completed' AND recorded_ms>=$3 AND recorded_ms<$4 AND recorded_ms<=$2),0)::bigint AS recorded_minutes_today
FROM tasks
