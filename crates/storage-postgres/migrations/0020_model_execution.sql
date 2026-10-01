ALTER TABLE reply_money_reservations DROP CONSTRAINT reply_money_reservations_request_kind_check;
ALTER TABLE reply_money_reservations ADD CONSTRAINT reply_money_reservations_request_kind_check
    CHECK (request_kind IN ('reply','model_planning','model_execution'));
CREATE TABLE model_execution_configurations (
    version text PRIMARY KEY CHECK (octet_length(version) BETWEEN 1 AND 128),
    configuration jsonb NOT NULL CHECK (octet_length(configuration::text)<=8192),
    disabled_at timestamptz
);
CREATE TABLE model_execution_requests (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    conversation_id uuid NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    data jsonb NOT NULL CHECK (octet_length(data::text)<=131072),
    PRIMARY KEY(user_id,request_id),
    UNIQUE(conversation_id,request_id)
);
-- 无正文的最小步骤状态，独立于对话清理，供共享金额审计关联每次付费尝试。
CREATE TABLE model_execution_call_audit (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    conversation_id uuid NOT NULL,
    request_id uuid NOT NULL,
    parent_request_id uuid NOT NULL,
    ordinal integer NOT NULL CHECK (ordinal BETWEEN 0 AND 3),
    status text NOT NULL CHECK(status IN ('reserved','dispatching','succeeded','failed','unknown','cancelled')),
    PRIMARY KEY(user_id,request_id)
);
