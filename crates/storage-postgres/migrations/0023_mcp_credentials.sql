CREATE TABLE mcp_credentials (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_digest text NOT NULL UNIQUE CHECK (token_digest ~ '^[0-9a-f]{64}$'),
    host_name text NOT NULL CHECK (length(host_name) BETWEEN 1 AND 80),
    scope text NOT NULL DEFAULT 'knowledge_search' CHECK (scope = 'knowledge_search'),
    created_at timestamptz NOT NULL DEFAULT NOW(),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    CHECK (expires_at > created_at AND expires_at <= created_at + INTERVAL '30 days')
);
CREATE INDEX mcp_credentials_owner ON mcp_credentials(user_id, created_at DESC);
