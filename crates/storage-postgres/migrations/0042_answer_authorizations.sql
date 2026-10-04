-- Source text stays in documents; terminal grants retain only bindings and audit metadata.
CREATE TABLE answer_authorizations (
 user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 request_id UUID NOT NULL CHECK(request_id <> '00000000-0000-0000-0000-000000000000'),
 endpoint TEXT NOT NULL CHECK(octet_length(endpoint) BETWEEN 1 AND 128),
 model TEXT NOT NULL CHECK(octet_length(model) BETWEEN 1 AND 128),
 question TEXT CHECK(length(question) BETWEEN 1 AND 1000),
 sources JSONB NOT NULL CHECK(jsonb_typeof(sources)='array' AND jsonb_array_length(sources) BETWEEN 1 AND 5 AND octet_length(sources::text)<=8192),
 intent_sha256 TEXT NOT NULL CHECK(intent_sha256 ~ '^[0-9a-f]{64}$'),
 digest TEXT NOT NULL CHECK(digest ~ '^[0-9a-f]{64}$'),
 status TEXT NOT NULL CHECK(status IN ('draft','authorized','cancelled','expired','invalidated','consumed')),
 created_ms BIGINT NOT NULL CHECK(created_ms>=0),
 expires_ms BIGINT NOT NULL CHECK(expires_ms=created_ms+600000),
 PRIMARY KEY(user_id,request_id),
 CHECK((status IN ('draft','authorized'))=(question IS NOT NULL))
);
CREATE INDEX answer_authorizations_owner_created ON answer_authorizations(user_id,created_ms DESC,request_id DESC);
CREATE TABLE answer_authorization_audit (
 user_id UUID NOT NULL, request_id UUID NOT NULL,
 event TEXT NOT NULL CHECK(event IN ('draft','authorized','cancelled','expired','invalidated','consumed')),
 at_ms BIGINT NOT NULL,
 PRIMARY KEY(user_id,request_id,event),
 FOREIGN KEY(user_id,request_id) REFERENCES answer_authorizations(user_id,request_id) ON DELETE CASCADE
);
CREATE FUNCTION audit_answer_authorization() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='INSERT' OR NEW.status IS DISTINCT FROM OLD.status THEN
  INSERT INTO answer_authorization_audit VALUES(NEW.user_id,NEW.request_id,NEW.status,floor(extract(epoch FROM clock_timestamp())*1000)::bigint);
 END IF;
 RETURN NEW;
END; $$;
CREATE TRIGGER answer_authorization_audit_trigger AFTER INSERT OR UPDATE ON answer_authorizations
 FOR EACH ROW EXECUTE FUNCTION audit_answer_authorization();
