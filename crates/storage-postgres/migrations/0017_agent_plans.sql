-- 固定只读检索计划；先预览，再精确授权，一次派发，最多三个工具步骤。
CREATE TABLE agent_plans (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    request_id UUID NOT NULL,
    revision BIGINT NOT NULL CHECK (revision BETWEEN 1 AND 100),
    digest TEXT NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
    status TEXT NOT NULL DEFAULT 'draft'
        CHECK (status IN ('draft','running','succeeded','failed','cancelled','stale','unknown')),
    call_limit INTEGER NOT NULL CHECK (call_limit BETWEEN 1 AND 3),
    attempted INTEGER NOT NULL DEFAULT 0 CHECK (attempted BETWEEN 0 AND 3),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()+interval '15 minutes',
    approved_at TIMESTAMPTZ,
    run_deadline TIMESTAMPTZ,
    PRIMARY KEY (user_id, conversation_id, request_id),
    CHECK (attempted <= call_limit),
    CHECK ((approved_at IS NULL) = (run_deadline IS NULL)),
    CHECK (status <> 'running' OR approved_at IS NOT NULL)
);
CREATE INDEX agent_plans_owner_conversation ON agent_plans(user_id, conversation_id, created_at);

CREATE TABLE agent_plan_steps (
    user_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    plan_id UUID NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 1 AND 3),
    call_id UUID NOT NULL,
    arguments JSONB NOT NULL CHECK (jsonb_typeof(arguments)='object' AND octet_length(arguments::text)<=8192),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','dispatching','succeeded','failed','cancelled')),
    output JSONB CHECK (octet_length(output::text)<=65536),
    PRIMARY KEY (user_id, conversation_id, plan_id, ordinal),
    UNIQUE (user_id, call_id),
    FOREIGN KEY (user_id, conversation_id, plan_id)
        REFERENCES agent_plans(user_id, conversation_id, request_id) ON DELETE CASCADE,
    CHECK ((status='succeeded') = (output IS NOT NULL))
);
