-- Keep the original authorization's consumed terminal state and exact v2 request binding.
-- Only the question is retained for reconstruction; source text stays in documents.
CREATE TABLE answer_executions (
 user_id UUID NOT NULL,
 request_id UUID NOT NULL,
 dispatch_token UUID NOT NULL UNIQUE CHECK(dispatch_token <> '00000000-0000-0000-0000-000000000000'),
 digest TEXT NOT NULL CHECK(digest ~ '^[0-9a-f]{64}$'),
 request_sha256 TEXT NOT NULL CHECK(request_sha256 ~ '^[0-9a-f]{64}$'),
 question TEXT CHECK(length(question) BETWEEN 1 AND 1000),
 status TEXT NOT NULL CHECK(status IN ('running','succeeded','failed','unknown','invalidated','cancelled')),
 started_ms BIGINT NOT NULL CHECK(started_ms>=0),
 deadline_ms BIGINT NOT NULL CHECK(deadline_ms>started_ms AND deadline_ms<=started_ms+90000),
 sent_ms BIGINT CHECK(sent_ms>=started_ms AND sent_ms<deadline_ms),
 finished_ms BIGINT CHECK(finished_ms>=started_ms),
 result JSONB CHECK(result IS NULL OR (jsonb_typeof(result)='object' AND octet_length(result::text)<=32768)),
 PRIMARY KEY(user_id,request_id),
 FOREIGN KEY(user_id,request_id) REFERENCES answer_authorizations(user_id,request_id) ON DELETE CASCADE,
 CHECK((status IN ('running','succeeded'))=(question IS NOT NULL)),
 CHECK((status='running')=(finished_ms IS NULL)),
 CHECK((status='succeeded')=(result IS NOT NULL)),
 CHECK(status<>'succeeded' OR sent_ms IS NOT NULL)
);
CREATE INDEX answer_executions_owner_started ON answer_executions(user_id,started_ms DESC,request_id DESC);
CREATE TABLE answer_execution_audit (
 user_id UUID NOT NULL,
 request_id UUID NOT NULL,
 event TEXT NOT NULL CHECK(event IN ('running','sending','succeeded','failed','unknown','invalidated','cancelled')),
 at_ms BIGINT NOT NULL CHECK(at_ms>=0),
 PRIMARY KEY(user_id,request_id,event),
 FOREIGN KEY(user_id,request_id) REFERENCES answer_executions(user_id,request_id) ON DELETE CASCADE
);
CREATE FUNCTION audit_answer_execution() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='INSERT' OR NEW.status IS DISTINCT FROM OLD.status THEN
  INSERT INTO answer_execution_audit VALUES(NEW.user_id,NEW.request_id,NEW.status,floor(extract(epoch FROM clock_timestamp())*1000)::bigint);
 END IF;
 IF NEW.sent_ms IS NOT NULL AND (TG_OP='INSERT' OR OLD.sent_ms IS NULL) THEN
  INSERT INTO answer_execution_audit VALUES(NEW.user_id,NEW.request_id,'sending',NEW.sent_ms);
 END IF;
 RETURN NEW;
END; $$;
CREATE TRIGGER answer_execution_audit_trigger AFTER INSERT OR UPDATE ON answer_executions
 FOR EACH ROW EXECUTE FUNCTION audit_answer_execution();
