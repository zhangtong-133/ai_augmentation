WITH RECURSIVE
 nodes AS (SELECT id,revision,enabled,deleted FROM learning_skills WHERE user_id=$1),
 edges AS (SELECT skill_id,prerequisite_id FROM learning_edges WHERE user_id=$1),
 reach(start,node) AS (
   SELECT skill_id,prerequisite_id FROM edges WHERE (SELECT count(*) FROM nodes)<=100
   UNION
   SELECT r.start,e.prerequisite_id FROM reach r JOIN edges e ON e.skill_id=r.node
 ),
 counts AS (
 SELECT 'skills' AS key,count(*) AS value FROM nodes
 UNION ALL SELECT 'deleted_skills',count(*) FROM nodes WHERE deleted
 UNION ALL SELECT 'enabled_skills',count(*) FROM nodes WHERE enabled AND NOT deleted
 UNION ALL SELECT 'edges',count(*) FROM edges
 UNION ALL SELECT 'cyclic_skills',count(*) FROM reach WHERE start=node
 UNION ALL SELECT 'skills_with_excess_prerequisites',count(*) FROM (SELECT skill_id FROM edges GROUP BY skill_id HAVING count(*)>8) e
 UNION ALL SELECT 'edges_on_deleted_skills',count(*) FROM edges e JOIN nodes n ON n.id=e.skill_id WHERE n.deleted
 UNION ALL SELECT 'unavailable_prerequisites',count(*) FROM edges e JOIN nodes n ON n.id=e.prerequisite_id WHERE n.deleted OR NOT n.enabled
 UNION ALL SELECT 'missing_edge_nodes',count(*) FROM edges e WHERE NOT EXISTS(SELECT 1 FROM nodes n WHERE n.id=e.skill_id) OR NOT EXISTS(SELECT 1 FROM nodes n WHERE n.id=e.prerequisite_id)
 UNION ALL SELECT 'assessments',count(*) FROM learning_assessments WHERE user_id=$1
 UNION ALL SELECT 'historical_assessments',count(*) FROM learning_assessments a JOIN nodes n ON n.id=a.skill_id WHERE a.user_id=$1 AND a.skill_revision<n.revision
 UNION ALL SELECT 'invalid_assessment_metadata',count(*) FROM learning_assessments a LEFT JOIN nodes n ON n.id=a.skill_id
   WHERE a.user_id=$1 AND (n.id IS NULL OR a.skill_revision>n.revision OR a.assessed_ms>$2 OR a.expected_revision>=COALESCE((SELECT revision FROM learning_state WHERE user_id=$1),0))
 UNION ALL SELECT 'revision_below_mutations',CASE WHEN COALESCE((SELECT revision FROM learning_state WHERE user_id=$1),0)<
   COALESCE((SELECT sum(revision) FROM nodes),0)+(SELECT count(*) FROM learning_assessments WHERE user_id=$1) THEN 1 ELSE 0 END
 UNION ALL SELECT 'plans',count(*) FROM learning_plans WHERE user_id=$1
 UNION ALL SELECT 'plans_today',count(*) FROM learning_plans WHERE user_id=$1 AND created_ms>=($2/86400000)*86400000
 UNION ALL SELECT 'ready_plans',count(*) FROM learning_plans WHERE user_id=$1 AND status='ready'
 UNION ALL SELECT 'invalidated_plans',count(*) FROM learning_plans WHERE user_id=$1 AND status='invalidated'
 UNION ALL SELECT 'deleted_plans',count(*) FROM learning_plans WHERE user_id=$1 AND status='deleted'
 UNION ALL SELECT 'historical_plans',count(*) FROM learning_plans WHERE user_id=$1 AND status='ready' AND snapshot_revision<COALESCE((SELECT revision FROM learning_state WHERE user_id=$1),0)
 UNION ALL SELECT 'tasks',count(*) FROM learning_tasks WHERE user_id=$1
 UNION ALL SELECT 'pending_tasks',count(*) FROM learning_tasks t WHERE t.user_id=$1 AND t.status='planned' AND NOT EXISTS(SELECT 1 FROM learning_results r WHERE r.user_id=t.user_id AND r.task_id=t.id)
 UNION ALL SELECT 'completed_results',count(*) FROM learning_results WHERE user_id=$1 AND outcome='completed'
 UNION ALL SELECT 'cancelled_results',count(*) FROM learning_results WHERE user_id=$1 AND outcome='cancelled'
 ) SELECT key,value FROM counts
