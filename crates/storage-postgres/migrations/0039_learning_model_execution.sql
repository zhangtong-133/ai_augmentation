ALTER TABLE learning_model_authorizations DROP CONSTRAINT learning_model_authorizations_status_check;
ALTER TABLE learning_model_authorizations ADD CHECK(status IN ('draft','authorized','running','succeeded','unknown','cancelled','expired','invalidated'));
ALTER TABLE learning_model_authorizations ADD COLUMN dispatch_token UUID;
ALTER TABLE learning_model_authorizations ADD COLUMN dispatch_deadline_ms BIGINT;
ALTER TABLE learning_model_authorizations ADD COLUMN sent_ms BIGINT;
ALTER TABLE learning_model_authorizations ADD COLUMN advice JSONB;
ALTER TABLE learning_model_authorizations ADD CHECK(status NOT IN ('running','succeeded') OR (approved_ms IS NOT NULL AND dispatch_token IS NOT NULL AND dispatch_deadline_ms IS NOT NULL));
ALTER TABLE learning_model_authorizations ADD CHECK((status='succeeded' AND advice IS NOT NULL AND sent_ms IS NOT NULL) OR (status<>'succeeded' AND advice IS NULL));
ALTER TABLE learning_model_authorizations ADD CHECK(advice IS NULL OR (jsonb_typeof(advice)='object' AND octet_length(advice::text)<=40000));
-- Invalidate derived advice too; never expand deletion to independent training records.
CREATE OR REPLACE FUNCTION invalidate_learning_model_authorizations() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 UPDATE learning_model_authorizations SET status='invalidated',advice=NULL
 WHERE user_id=OLD.user_id AND status IN ('draft','authorized','running','succeeded') AND
  CASE TG_TABLE_NAME
   WHEN 'learning_evidence' THEN task_id=(to_jsonb(OLD)->>'task_id')::uuid
   WHEN 'learning_plans' THEN plan_id=(to_jsonb(OLD)->>'request_id')::uuid
   WHEN 'subscription_connections' THEN connection_id=(to_jsonb(OLD)->>'id')::uuid
   ELSE TRUE END;
 RETURN OLD;
END; $$;
