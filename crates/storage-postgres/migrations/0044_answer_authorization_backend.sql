-- Existing request bytes and digests remain bound to the local v2 protocol.
ALTER TABLE answer_authorizations
  ADD COLUMN backend TEXT NOT NULL DEFAULT 'local'
  CHECK (backend IN ('local', 'ollama'));
