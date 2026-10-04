-- Synthetic schema-state fixtures. No real credentials, content or providers.
DO $$ DECLARE
 owner_id uuid := '00000000-0000-4000-8000-000000000001';
 conversation_id uuid := gen_random_uuid(); second_conversation uuid := gen_random_uuid();
 document_id uuid := gen_random_uuid(); subscription_id uuid := gen_random_uuid();
 stamp bigint := floor(extract(epoch FROM clock_timestamp())*1000)::bigint;
 request_id uuid; step_id uuid := gen_random_uuid();
BEGIN
 INSERT INTO sessions(token_digest,user_id) VALUES('synthetic-old-session',owner_id);
 INSERT INTO mcp_credentials(id,user_id,token_digest,host_name,created_at,expires_at)
 VALUES(gen_random_uuid(),owner_id,repeat('a',64),'synthetic host',now(),now()+interval '1 day');
 INSERT INTO documents(id,user_id,title,source,tags,content_digest,markdown,chunks,created_at_unix_ms)
 VALUES(document_id,owner_id,'合成文档','markdown','{}','synthetic-content','合成正文',ARRAY['合成正文'],stamp);
 INSERT INTO document_index_jobs(document_id,target,status,total_chunks) VALUES(document_id,'synthetic','queued',1);
 INSERT INTO conversations(id,user_id,request_id,title)
 VALUES(conversation_id,owner_id,gen_random_uuid(),'合成会话'),(second_conversation,owner_id,gen_random_uuid(),'合成会话二');
 INSERT INTO reply_daily_budgets(user_id,day,reserved) VALUES(owner_id,current_date,2);
 INSERT INTO conversation_replies(conversation_id,request_id,revision,budget_day,status,dispatched_at)
 VALUES(conversation_id,gen_random_uuid(),1,current_date,'queued',NULL),
 (second_conversation,gen_random_uuid(),1,current_date,'dispatching',now());
 INSERT INTO reply_configurations(revision,model,budget,valid_until_ms) VALUES('synthetic','synthetic','{}',stamp+3600000);
 INSERT INTO model_planning_configurations(version,configuration,valid_until_ms) VALUES('synthetic','{}',stamp+3600000);
 INSERT INTO model_execution_configurations(version,configuration) VALUES('synthetic','{}');
 INSERT INTO reply_money_daily(user_id,day,currency,occupied) VALUES(owner_id,current_date,'USD',100);
 INSERT INTO reply_money_reservations(user_id,conversation_id,request_id,day,currency,model,configuration_revision,budget,reserved)
 VALUES(owner_id,conversation_id,gen_random_uuid(),current_date,'USD','synthetic','synthetic','{}',100);
 INSERT INTO model_agent_daily(user_id,day,occupied) VALUES(owner_id,current_date,1);
 INSERT INTO model_planning_requests(user_id,conversation_id,request_id,revision,version,configuration_version,configuration,snapshot,digest,currency,amount,status,created_at_ms,expires_at_ms)
 VALUES(owner_id,conversation_id,gen_random_uuid(),1,'synthetic','synthetic','{}','{}',repeat('a',64),'USD',1,'draft',stamp,stamp+300000);
 request_id := gen_random_uuid();
 INSERT INTO model_execution_requests(user_id,conversation_id,request_id,data)
 VALUES(owner_id,conversation_id,request_id,jsonb_build_object('request',jsonb_build_object('status','queued','searches',jsonb_build_array(),'evidence',jsonb_build_array(),'answer',NULL),
 'steps',jsonb_build_array(jsonb_build_object('call_id',step_id,'status','reserved'))));
 INSERT INTO model_execution_call_audit(user_id,conversation_id,request_id,parent_request_id,ordinal,status)
 VALUES(owner_id,conversation_id,step_id,request_id,0,'reserved');
 INSERT INTO agent_plans(user_id,conversation_id,request_id,revision,digest,status,call_limit)
 VALUES(owner_id,conversation_id,gen_random_uuid(),1,repeat('a',64),'draft',1);
 INSERT INTO tool_daily_budgets(user_id,day,used) VALUES(owner_id,current_date,1);
 INSERT INTO tool_calls(user_id,request_id,tool,arguments_digest,day,status,input_bytes)
 VALUES(owner_id,gen_random_uuid(),'synthetic',repeat('a',64),current_date,'running',2);
 INSERT INTO schedules(user_id,request_id,version,title,body,run_at_ms,digest,status,created_ms,approval_expires_ms)
 VALUES(owner_id,gen_random_uuid(),'local-reminder-once-v1','合成提醒','合成正文',stamp+600000,repeat('a',64),'draft',stamp,stamp+300000);
 INSERT INTO feed_brief_schedules(user_id,revision,enabled,minute_utc,next_run_ms)
 VALUES(owner_id,1,true,0,stamp+600000);
 INSERT INTO feed_subscriptions(user_id,id,name,source_url,enabled)
 VALUES(owner_id,subscription_id,'合成订阅','https://example.invalid/feed',true);
 INSERT INTO feed_schedules(user_id,id,subscription_id,plan,digest,status,created_ms,approval_expires_ms,ends_ms,approved_ms)
 VALUES(owner_id,gen_random_uuid(),subscription_id,'{}',repeat('a',64),'active',stamp,stamp+300000,stamp+600000,stamp+1);
 INSERT INTO feed_collections(user_id,request_id,subscription_id,plan,digest,status,created_ms)
 VALUES(owner_id,gen_random_uuid(),subscription_id,'{}',repeat('a',64),'draft',stamp);
 INSERT INTO feed_collections(user_id,request_id,subscription_id,plan,digest,status,created_ms,claimed_ms,deadline_ms,claim_id,accepted_digest)
 VALUES(owner_id,gen_random_uuid(),subscription_id,'{}',repeat('a',64),'running',stamp,stamp+1,stamp+60001,gen_random_uuid(),repeat('a',64));
 INSERT INTO feed_value_reviews(user_id,id,status,snapshot,pricing,digest,created_ms,expires_ms,approved_ms)
 VALUES(owner_id,gen_random_uuid(),'authorized','{}','{}',repeat('a',64),stamp,stamp+300000,stamp+1);
 INSERT INTO feed_value_reviews(user_id,id,status,snapshot,pricing,digest,created_ms,expires_ms,approved_ms,dispatch_token,dispatch_deadline_ms,sent_ms)
 VALUES(owner_id,gen_random_uuid(),'running','{}','{"kind":"subscription"}',repeat('a',64),stamp,stamp+300000,stamp+1,gen_random_uuid(),stamp+90000,stamp+2);
 INSERT INTO learning_model_authorizations(user_id,request_id,plan_id,task_id,model,input_digest,digest,status,created_ms,expires_ms,approved_ms,local_endpoint)
 VALUES(owner_id,gen_random_uuid(),gen_random_uuid(),gen_random_uuid(),'qwen3:4b-q4_K_M',repeat('a',64),repeat('b',64),'authorized',stamp,stamp+300000,stamp+1,'http://127.0.0.1:11435');
 INSERT INTO learning_model_authorizations(user_id,request_id,plan_id,task_id,model,input_digest,digest,status,created_ms,expires_ms,approved_ms,local_endpoint,dispatch_token,dispatch_deadline_ms,sent_ms)
 VALUES(owner_id,gen_random_uuid(),gen_random_uuid(),gen_random_uuid(),'qwen3:4b-q4_K_M',repeat('a',64),repeat('b',64),'running',stamp,stamp+300000,stamp+1,'http://127.0.0.1:11435',gen_random_uuid(),stamp+90000,stamp+2);
END $$;
