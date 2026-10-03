-- Only source/connection bindings and consent metadata; evidence remains in its original table.
CREATE TABLE learning_model_authorizations (
 user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 request_id UUID NOT NULL CHECK(request_id<>'00000000-0000-0000-0000-000000000000'),
 plan_id UUID NOT NULL, task_id UUID NOT NULL, connection_id UUID NOT NULL,
 connection_revision BIGINT NOT NULL CHECK(connection_revision>0),
 model TEXT NOT NULL CHECK(octet_length(model) BETWEEN 1 AND 128),
 input_digest TEXT NOT NULL CHECK(input_digest ~ '^[0-9a-f]{64}$'),
 digest TEXT NOT NULL CHECK(digest ~ '^[0-9a-f]{64}$'),
 status TEXT NOT NULL CHECK(status IN ('draft','authorized','cancelled','expired','invalidated')),
 created_ms BIGINT NOT NULL CHECK(created_ms>=0),
 expires_ms BIGINT NOT NULL CHECK(expires_ms>created_ms AND expires_ms<=created_ms+300000),
 approved_ms BIGINT CHECK(approved_ms>=created_ms AND approved_ms<expires_ms),
 PRIMARY KEY(user_id,request_id),
 CHECK(status<>'authorized' OR approved_ms IS NOT NULL)
);
CREATE TABLE learning_model_authorization_audit (
 user_id UUID NOT NULL, request_id UUID NOT NULL, event TEXT NOT NULL, at_ms BIGINT NOT NULL,
 PRIMARY KEY(user_id,request_id,event),
 FOREIGN KEY(user_id,request_id) REFERENCES learning_model_authorizations(user_id,request_id) ON DELETE CASCADE
);
CREATE FUNCTION audit_learning_model_authorization() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='INSERT' OR NEW.status IS DISTINCT FROM OLD.status THEN
  INSERT INTO learning_model_authorization_audit VALUES(NEW.user_id,NEW.request_id,NEW.status,floor(extract(epoch FROM clock_timestamp())*1000)::bigint);
 END IF;
 RETURN NEW;
END; $$;
CREATE TRIGGER learning_model_authorization_audit AFTER INSERT OR UPDATE ON learning_model_authorizations
 FOR EACH ROW EXECUTE FUNCTION audit_learning_model_authorization();
CREATE FUNCTION invalidate_learning_model_authorizations() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 UPDATE learning_model_authorizations SET status='invalidated'
 WHERE user_id=OLD.user_id AND status IN ('draft','authorized') AND
  CASE TG_TABLE_NAME
   WHEN 'learning_evidence' THEN task_id=(to_jsonb(OLD)->>'task_id')::uuid
   WHEN 'learning_plans' THEN plan_id=(to_jsonb(OLD)->>'request_id')::uuid
   WHEN 'subscription_connections' THEN connection_id=(to_jsonb(OLD)->>'id')::uuid
   ELSE TRUE END;
 RETURN OLD;
END; $$;
CREATE TRIGGER learning_model_evidence_change AFTER UPDATE OR DELETE ON learning_evidence FOR EACH ROW EXECUTE FUNCTION invalidate_learning_model_authorizations();
CREATE TRIGGER learning_model_plan_change AFTER UPDATE OR DELETE ON learning_plans FOR EACH ROW EXECUTE FUNCTION invalidate_learning_model_authorizations();
CREATE TRIGGER learning_model_skill_change AFTER UPDATE OR DELETE ON learning_skills FOR EACH ROW EXECUTE FUNCTION invalidate_learning_model_authorizations();
CREATE TRIGGER learning_model_assessment_change AFTER UPDATE OR DELETE ON learning_assessments FOR EACH ROW EXECUTE FUNCTION invalidate_learning_model_authorizations();
CREATE TRIGGER learning_model_connection_change AFTER UPDATE OR DELETE ON subscription_connections FOR EACH ROW EXECUTE FUNCTION invalidate_learning_model_authorizations();
