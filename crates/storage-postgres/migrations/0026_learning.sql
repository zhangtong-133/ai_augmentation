CREATE TABLE learning_state (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL CHECK(revision>0)
);
CREATE TABLE learning_skills (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    id UUID NOT NULL CHECK(id<>'00000000-0000-0000-0000-000000000000'),
    revision BIGINT NOT NULL CHECK(revision>0),
    name TEXT NOT NULL CHECK(char_length(name) BETWEEN 1 AND 120 AND octet_length(name)<=480),
    enabled BOOLEAN NOT NULL,
    deleted BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY(user_id,id),
    CHECK(NOT deleted OR (NOT enabled AND name='已删除技能'))
);
CREATE TABLE learning_edges (
    user_id UUID NOT NULL,
    skill_id UUID NOT NULL,
    prerequisite_id UUID NOT NULL,
    PRIMARY KEY(user_id,skill_id,prerequisite_id),
    FOREIGN KEY(user_id,skill_id) REFERENCES learning_skills(user_id,id) ON DELETE CASCADE,
    FOREIGN KEY(user_id,prerequisite_id) REFERENCES learning_skills(user_id,id) DEFERRABLE INITIALLY DEFERRED,
    CHECK(skill_id<>prerequisite_id)
);
CREATE TABLE learning_assessments (
    user_id UUID NOT NULL,
    id UUID NOT NULL CHECK(id<>'00000000-0000-0000-0000-000000000000'),
    skill_id UUID NOT NULL,
    skill_revision BIGINT NOT NULL CHECK(skill_revision>0),
    expected_revision BIGINT NOT NULL CHECK(expected_revision>=0),
    score SMALLINT CHECK(score BETWEEN 0 AND 100),
    assessed_ms BIGINT NOT NULL CHECK(assessed_ms>=0),
    PRIMARY KEY(user_id,id),
    FOREIGN KEY(user_id,skill_id) REFERENCES learning_skills(user_id,id) ON DELETE CASCADE,
    UNIQUE(user_id,skill_id,skill_revision,assessed_ms)
);
CREATE TABLE learning_plans (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    request_id UUID NOT NULL CHECK(request_id<>'00000000-0000-0000-0000-000000000000'),
    snapshot_revision BIGINT NOT NULL CHECK(snapshot_revision>=0),
    request_digest TEXT NOT NULL CHECK(request_digest ~ '^[0-9a-f]{64}$'),
    digest TEXT NOT NULL CHECK(digest ~ '^[0-9a-f]{64}$'),
    created_ms BIGINT NOT NULL CHECK(created_ms>=0),
    status TEXT NOT NULL CHECK(status IN ('ready','invalidated','deleted')),
    plan JSONB CHECK(octet_length(plan::text)<=2097152),
    PRIMARY KEY(user_id,request_id),
    CHECK((status='ready')=(plan IS NOT NULL))
);
CREATE INDEX learning_plan_daily ON learning_plans(user_id,created_ms);
CREATE TABLE learning_plan_sources (
    user_id UUID NOT NULL,
    request_id UUID NOT NULL,
    skill_id UUID NOT NULL,
    PRIMARY KEY(user_id,request_id,skill_id),
    FOREIGN KEY(user_id,request_id) REFERENCES learning_plans(user_id,request_id) ON DELETE CASCADE,
    FOREIGN KEY(user_id,skill_id) REFERENCES learning_skills(user_id,id) ON DELETE CASCADE
);
CREATE INDEX learning_plan_source_lookup ON learning_plan_sources(user_id,skill_id);
CREATE TABLE learning_tasks (
    user_id UUID NOT NULL,
    request_id UUID NOT NULL,
    id UUID NOT NULL,
    ordinal SMALLINT NOT NULL CHECK(ordinal BETWEEN 0 AND 4),
    status TEXT NOT NULL CHECK(status IN ('planned','invalidated','deleted')),
    task JSONB CHECK(octet_length(task::text)<=16384),
    PRIMARY KEY(user_id,id),
    UNIQUE(user_id,request_id,ordinal),
    FOREIGN KEY(user_id,request_id) REFERENCES learning_plans(user_id,request_id) ON DELETE CASCADE,
    CHECK((status='planned')=(task IS NOT NULL))
);
CREATE FUNCTION invalidate_learning_plans() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR (NEW.deleted AND NOT OLD.deleted) THEN
        UPDATE learning_assessments SET score=NULL WHERE user_id=OLD.user_id AND skill_id=OLD.id;
        UPDATE learning_plans p SET status='invalidated',plan=NULL
        WHERE p.user_id=OLD.user_id AND p.status='ready' AND EXISTS (
            SELECT 1 FROM learning_plan_sources s WHERE s.user_id=p.user_id AND s.request_id=p.request_id AND s.skill_id=OLD.id
        );
        UPDATE learning_tasks t SET status='invalidated',task=NULL
        WHERE t.user_id=OLD.user_id AND EXISTS (
            SELECT 1 FROM learning_plans p JOIN learning_plan_sources s USING(user_id,request_id)
            WHERE p.user_id=t.user_id AND p.request_id=t.request_id AND p.status='invalidated' AND s.skill_id=OLD.id
        );
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER learning_delete_cleanup BEFORE UPDATE OF deleted OR DELETE ON learning_skills
    FOR EACH ROW EXECUTE FUNCTION invalidate_learning_plans();
