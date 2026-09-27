-- 金额账本和最小结算凭据独立于对话墓碑；仅删除用户时级联清理。
CREATE TABLE reply_money_daily (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    day date NOT NULL,
    currency text NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    occupied bigint NOT NULL CHECK (occupied >= 0),
    PRIMARY KEY (user_id, day, currency)
);

CREATE TABLE reply_money_reservations (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    conversation_id uuid NOT NULL,
    request_id uuid NOT NULL,
    day date NOT NULL,
    currency text NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    model text NOT NULL CHECK (octet_length(model) BETWEEN 1 AND 128),
    configuration_revision text NOT NULL CHECK (octet_length(configuration_revision) BETWEEN 1 AND 128),
    budget jsonb NOT NULL CHECK (octet_length(budget::text) <= 4096),
    reserved bigint NOT NULL CHECK (reserved > 0),
    charged bigint CHECK (charged BETWEEN 0 AND reserved),
    settlement text CHECK (settlement IN ('cancelled_before_dispatch','verified','retained','usage_exceeded')),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    settled_at timestamptz,
    PRIMARY KEY (conversation_id, request_id),
    CHECK ((charged IS NULL AND settlement IS NULL AND settled_at IS NULL) OR
           (charged IS NOT NULL AND settlement IS NOT NULL AND settled_at IS NOT NULL))
);
CREATE INDEX reply_money_reservations_owner ON reply_money_reservations(user_id, day, currency);
