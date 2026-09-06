CREATE TABLE documents (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 200),
    source TEXT NOT NULL,
    tags TEXT[] NOT NULL,
    content_digest TEXT NOT NULL,
    markdown TEXT NOT NULL,
    chunks TEXT[] NOT NULL CHECK (cardinality(chunks) > 0),
    created_at_unix_ms BIGINT NOT NULL,
    UNIQUE (user_id, content_digest)
);
CREATE INDEX documents_owner_created_idx ON documents(user_id, created_at_unix_ms DESC, id DESC);
