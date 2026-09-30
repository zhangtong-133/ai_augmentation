-- 配置版本不可覆盖或重新启用；保留跨进程停用凭据。
CREATE TABLE reply_configurations (
    revision text PRIMARY KEY CHECK (octet_length(revision) BETWEEN 1 AND 128),
    model text NOT NULL CHECK (octet_length(model) BETWEEN 1 AND 128),
    budget jsonb NOT NULL CHECK (octet_length(budget::text) <= 4096),
    valid_until_ms bigint NOT NULL CHECK (valid_until_ms > 0),
    disabled_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
