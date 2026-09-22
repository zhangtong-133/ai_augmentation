-- 独立额度账本不随对话删除，避免删除/重建绕过每日次数限制。
CREATE TABLE reply_daily_budgets (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    day date NOT NULL,
    reserved integer NOT NULL CHECK (reserved BETWEEN 0 AND 20),
    PRIMARY KEY (user_id, day)
);

CREATE TABLE conversation_replies (
    conversation_id uuid NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    revision bigint NOT NULL CHECK (revision BETWEEN 1 AND 100),
    budget_day date NOT NULL,
    status text NOT NULL CHECK (status IN ('queued','dispatching','succeeded','failed','unknown','cancelled')),
    context jsonb,
    output text CHECK (octet_length(output) BETWEEN 1 AND 16384),
    dispatched_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (conversation_id, request_id),
    CHECK (context IS NULL OR octet_length(context::text) <= 131072),
    CHECK (output IS NULL OR status = 'succeeded'),
    CHECK (status NOT IN ('dispatching','succeeded','failed','unknown') OR dispatched_at IS NOT NULL),
    CHECK (status <> 'queued' OR dispatched_at IS NULL)
);
CREATE UNIQUE INDEX conversation_replies_active ON conversation_replies(conversation_id)
    WHERE status IN ('queued','dispatching');
