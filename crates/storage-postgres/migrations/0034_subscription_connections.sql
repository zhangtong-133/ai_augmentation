-- Metadata only. OAuth credentials remain in the protected local runtime.
CREATE TABLE subscription_connections (
 user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 id UUID NOT NULL CHECK(id<>'00000000-0000-0000-0000-000000000000'),
 host_id UUID NOT NULL,
 client_id TEXT NOT NULL CHECK(octet_length(client_id) BETWEEN 1 AND 256),
 subject_hash TEXT NOT NULL CHECK(subject_hash ~ '^[0-9a-f]{64}$'),
 label TEXT NOT NULL CHECK(octet_length(label) BETWEEN 1 AND 64),
 models JSONB NOT NULL CHECK(jsonb_typeof(models)='array' AND jsonb_array_length(models) BETWEEN 1 AND 100 AND octet_length(models::text)<=16384),
 revision BIGINT NOT NULL CHECK(revision BETWEEN 1 AND 1000),
 status TEXT NOT NULL CHECK(status IN ('active','revoked','expired')),
 expires_ms BIGINT NOT NULL CHECK(expires_ms>0),
 PRIMARY KEY(user_id,id),
 UNIQUE(host_id,client_id)
);
CREATE TABLE subscription_connection_audit (
 user_id UUID NOT NULL,
 connection_id UUID NOT NULL,
 revision BIGINT NOT NULL,
 status TEXT NOT NULL,
 at_ms BIGINT NOT NULL,
 PRIMARY KEY(user_id,connection_id,revision),
 FOREIGN KEY(user_id,connection_id) REFERENCES subscription_connections(user_id,id) ON DELETE CASCADE
);
CREATE FUNCTION subscription_connection_changed() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='INSERT' OR NEW.revision IS DISTINCT FROM OLD.revision THEN
  INSERT INTO subscription_connection_audit(user_id,connection_id,revision,status,at_ms)
   VALUES(NEW.user_id,NEW.id,NEW.revision,NEW.status,floor(extract(epoch FROM clock_timestamp())*1000)::bigint);
 END IF;
 IF TG_OP='UPDATE' THEN
  UPDATE feed_value_reviews SET status='invalidated',snapshot=NULL
   WHERE user_id=NEW.user_id AND status IN ('draft','authorized')
    AND pricing->>'kind'='subscription' AND pricing->>'connection_id'=NEW.id::text;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER subscription_connection_state AFTER INSERT OR UPDATE ON subscription_connections
 FOR EACH ROW EXECUTE FUNCTION subscription_connection_changed();
-- Legacy previews have no verified owner-bound connection and must be authorized anew.
UPDATE feed_value_reviews SET status='invalidated',snapshot=NULL
 WHERE status IN ('draft','authorized') AND pricing->>'kind'='subscription';
