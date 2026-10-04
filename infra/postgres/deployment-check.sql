BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout='10s';
SELECT jsonb_build_object(
 'postgresMajor',current_setting('server_version_num')::int/10000,
 'failedMigrations',(SELECT count(*) FROM _sqlx_migrations WHERE NOT success),
 'migrations',(SELECT jsonb_agg(jsonb_build_object('version',version,'checksum',encode(checksum,'hex')) ORDER BY version) FROM _sqlx_migrations WHERE success),
 'sessions',(SELECT count(*) FROM sessions),
 'activeMcp',(SELECT count(*) FROM mcp_credentials WHERE revoked_at IS NULL),
 'activeConfigurations',(SELECT count(*) FROM reply_configurations WHERE disabled_at IS NULL)
  +(SELECT count(*) FROM model_planning_configurations WHERE disabled_at IS NULL)
  +(SELECT count(*) FROM model_execution_configurations WHERE disabled_at IS NULL),
 'unsettledMoney',(SELECT count(*) FROM reply_money_reservations WHERE settled_at IS NULL),
 'activeJobs',(SELECT count(*) FROM document_index_jobs WHERE status IN ('queued','running','retrying'))
  +(SELECT count(*) FROM conversation_replies WHERE status IN ('queued','dispatching'))
  +(SELECT count(*) FROM model_planning_requests WHERE status IN ('draft','queued','dispatching'))
  +(SELECT count(*) FROM model_execution_requests WHERE data#>>'{request,status}' IN ('draft','queued','running'))
  +(SELECT count(*) FROM model_execution_call_audit WHERE status IN ('reserved','dispatching'))
  +(SELECT count(*) FROM agent_plans WHERE status IN ('draft','running'))
  +(SELECT count(*) FROM tool_calls WHERE status='running')
  +(SELECT count(*) FROM schedules WHERE status IN ('draft','scheduled','running'))
  +(SELECT count(*) FROM feed_brief_schedules WHERE enabled)
  +(SELECT count(*) FROM feed_schedules WHERE status IN ('draft','active'))
  +(SELECT count(*) FROM feed_collections WHERE status IN ('draft','running'))
  +(SELECT count(*) FROM feed_value_reviews WHERE status IN ('draft','authorized','running'))
  +(SELECT count(*) FROM learning_model_authorizations WHERE status IN ('draft','authorized','running')),
 'unknownModelRequests',(SELECT count(*) FROM learning_model_authorizations WHERE status='unknown')
  +(SELECT count(*) FROM feed_value_reviews WHERE status='unknown')
  +(SELECT count(*) FROM model_planning_requests WHERE status='unknown')
  +(SELECT count(*) FROM model_execution_requests WHERE data#>>'{request,status}'='unknown')
  +(SELECT count(*) FROM conversation_replies WHERE status='unknown'),
 'externalOriginals',(SELECT count(*) FROM documents WHERE original_object_key IS NOT NULL),
 'originalRefs',CASE WHEN (SELECT count(*) FROM documents WHERE original_object_key IS NOT NULL)<=1000 THEN
  (SELECT COALESCE(jsonb_agg(jsonb_build_object('key',original_object_key,'sourceType',source_type) ORDER BY original_object_key),'[]'::jsonb) FROM documents WHERE original_object_key IS NOT NULL) ELSE NULL END
);
ROLLBACK;
