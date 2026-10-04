SELECT jsonb_build_object(
 'users',(SELECT count(*) FROM users),
 'documents',(SELECT count(*) FROM documents),
 'sessions',(SELECT count(*) FROM sessions),
 'activeMcp',(SELECT count(*) FROM mcp_credentials WHERE revoked_at IS NULL),
 'activeConfigurations',(SELECT count(*) FROM reply_configurations WHERE disabled_at IS NULL)
   +(SELECT count(*) FROM model_planning_configurations WHERE disabled_at IS NULL)
   +(SELECT count(*) FROM model_execution_configurations WHERE disabled_at IS NULL),
 'unsettledMoney',(SELECT count(*) FROM reply_money_reservations WHERE settled_at IS NULL),
 'occupiedMoney',(SELECT sum(occupied) FROM reply_money_daily),
 'retainedMoney',(SELECT sum(charged) FROM reply_money_reservations WHERE settlement='retained'),
 'occupiedCalls',(SELECT sum(occupied) FROM model_agent_daily),
 'toolCalls',(SELECT sum(used) FROM tool_daily_budgets),
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
 'learningStates',(SELECT jsonb_agg(status ORDER BY status) FROM learning_model_authorizations),
 'unknownSending',(SELECT count(*) FROM learning_model_authorizations WHERE status='unknown' AND sent_ms IS NOT NULL),
 'sendingAudits',(SELECT count(*) FROM learning_model_authorization_audit WHERE event='unknown')
);
