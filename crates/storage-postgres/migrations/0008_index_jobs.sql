CREATE TABLE document_index_jobs (
    document_id UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    target TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'retrying', 'completed', 'failed')),
    indexed_chunks INTEGER NOT NULL DEFAULT 0 CHECK (indexed_chunks >= 0),
    total_chunks INTEGER NOT NULL CHECK (total_chunks > 0 AND indexed_chunks <= total_chunks),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 3),
    error_code TEXT,
    lease UUID,
    lease_until TIMESTAMPTZ,
    available_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (document_id, target)
);
CREATE INDEX document_index_jobs_ready ON document_index_jobs(target, available_at)
    WHERE status IN ('queued', 'retrying', 'running');
