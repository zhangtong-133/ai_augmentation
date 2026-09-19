-- 复合外键确保任务的 owner 与文档 owner 一致。
CREATE UNIQUE INDEX documents_owner_id_unique ON documents(user_id, id);
CREATE TABLE document_index_jobs (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL,
    document_id UUID NOT NULL,
    profile TEXT NOT NULL CHECK (profile ~ '^[0-9a-f]{64}$'),
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued','running','succeeded','failed')),
    indexed_chunks INTEGER NOT NULL DEFAULT 0,
    total_chunks INTEGER NOT NULL CHECK (total_chunks > 0),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 5),
    error_code TEXT,
    available_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    lease_until TIMESTAMPTZ,
    lease_token UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, document_id, profile),
    FOREIGN KEY(user_id, document_id) REFERENCES documents(user_id, id) ON DELETE CASCADE,
    CHECK (indexed_chunks BETWEEN 0 AND total_chunks),
    CHECK ((status = 'succeeded') = (indexed_chunks = total_chunks)),
    CHECK ((status = 'running' AND lease_until IS NOT NULL AND lease_token IS NOT NULL)
        OR (status <> 'running' AND lease_until IS NULL AND lease_token IS NULL))
);
CREATE INDEX document_index_jobs_ready ON document_index_jobs(profile, available_at, id)
    WHERE status IN ('queued','running');
