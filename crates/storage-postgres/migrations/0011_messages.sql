ALTER TABLE conversations ADD COLUMN message_revision bigint NOT NULL DEFAULT 0 CHECK (message_revision >= 0);
ALTER TABLE conversations ADD COLUMN cache_delete_pending boolean NOT NULL DEFAULT false;

CREATE TABLE conversation_messages (
    id uuid PRIMARY KEY,
    conversation_id uuid NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    sequence bigint NOT NULL CHECK (sequence BETWEEN 1 AND 100),
    content text NOT NULL CHECK (octet_length(content) BETWEEN 1 AND 4096),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (conversation_id, request_id),
    UNIQUE (conversation_id, sequence)
);
CREATE INDEX conversations_cache_deletions ON conversations(deleted_at) WHERE cache_delete_pending;
