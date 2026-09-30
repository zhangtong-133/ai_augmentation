-- 规划金额复用同币种日账本，凭据类型避免无金额回复误结算规划预算。
ALTER TABLE reply_money_reservations ADD COLUMN request_kind text NOT NULL DEFAULT 'reply'
    CHECK (request_kind IN ('reply','model_planning'));

CREATE TABLE model_planning_configurations (
    version text PRIMARY KEY CHECK (octet_length(version) BETWEEN 1 AND 128),
    configuration jsonb NOT NULL CHECK (octet_length(configuration::text) <= 8192),
    valid_until_ms bigint NOT NULL CHECK (valid_until_ms > 0),
    disabled_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

-- 未派发预留和已派发尝试共用占用量；删除对话不能重置次数。
CREATE TABLE model_agent_daily (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    day date NOT NULL,
    occupied integer NOT NULL CHECK (occupied >= 0),
    PRIMARY KEY (user_id,day)
);

CREATE TABLE model_planning_requests (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    conversation_id uuid NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    revision bigint NOT NULL CHECK (revision BETWEEN 1 AND 100),
    version text NOT NULL,
    configuration_version text NOT NULL,
    configuration jsonb NOT NULL CHECK (octet_length(configuration::text) <= 8192),
    -- 100 × 4096 字节的合法正文在 JSON 控制字符转义后最多扩张六倍。
    snapshot jsonb CHECK (octet_length(snapshot::text) <= 3145728),
    digest text NOT NULL CHECK (digest ~ '^[a-f0-9]{64}$'),
    currency text NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    amount bigint NOT NULL CHECK (amount > 0),
    status text NOT NULL CHECK (status IN ('draft','queued','dispatching','succeeded','failed','unknown','cancelled','expired','stale')),
    approval jsonb CHECK (octet_length(approval::text) <= 4096),
    searches jsonb CHECK (octet_length(searches::text) <= 16384),
    claim_id uuid,
    budget_day date,
    created_at_ms bigint NOT NULL,
    expires_at_ms bigint NOT NULL CHECK (expires_at_ms > created_at_ms),
    approved_at_ms bigint,
    deadline_ms bigint,
    PRIMARY KEY (user_id,request_id),
    UNIQUE (conversation_id,request_id),
    CHECK (searches IS NULL OR status='succeeded'),
    CHECK (status NOT IN ('queued','dispatching') OR (approval IS NOT NULL AND budget_day IS NOT NULL)),
    CHECK (status <> 'dispatching' OR (claim_id IS NOT NULL AND deadline_ms IS NOT NULL))
);
CREATE INDEX model_planning_requests_owner ON model_planning_requests(user_id,conversation_id,created_at_ms DESC,request_id);
CREATE INDEX model_planning_requests_deadline ON model_planning_requests(user_id,conversation_id,status,expires_at_ms,deadline_ms);
