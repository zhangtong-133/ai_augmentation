-- Compatible with the original Compose init script. SQLx owns migration history.
CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT users_email_not_blank CHECK (length(trim(email)) > 0),
    CONSTRAINT users_display_name_not_blank CHECK (length(trim(display_name)) > 0)
);
CREATE INDEX IF NOT EXISTS users_created_at_idx ON users (created_at DESC);
