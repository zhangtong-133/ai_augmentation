ALTER TABLE users ADD COLUMN password_hash TEXT;
CREATE TABLE sessions (
    token_digest TEXT PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '8 hours')
);
CREATE INDEX sessions_user_idx ON sessions(user_id);
CREATE INDEX sessions_expiry_idx ON sessions(expires_at);
