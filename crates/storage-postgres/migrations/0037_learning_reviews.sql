ALTER TABLE learning_evidence ADD UNIQUE(user_id,task_id,request_id);
CREATE TABLE learning_reviews (
    user_id UUID NOT NULL,
    task_id UUID NOT NULL,
    request_id UUID NOT NULL CHECK(request_id<>'00000000-0000-0000-0000-000000000000'),
    evidence_request_id UUID NOT NULL,
    rubric_version TEXT NOT NULL CHECK(rubric_version='user-evidence-review-v1'),
    body JSONB,
    status TEXT NOT NULL CHECK(status IN ('pending','confirmed','invalidated')),
    created_ms BIGINT NOT NULL CHECK(created_ms>=0),
    confirmation_request_id UUID CHECK(confirmation_request_id<>'00000000-0000-0000-0000-000000000000'),
    assessment_id UUID,
    expected_revision BIGINT CHECK(expected_revision>=0),
    confirmed_score SMALLINT CHECK(confirmed_score BETWEEN 0 AND 100),
    PRIMARY KEY(user_id,task_id),
    UNIQUE(user_id,request_id),
    UNIQUE(user_id,confirmation_request_id),
    FOREIGN KEY(user_id,task_id,evidence_request_id) REFERENCES learning_evidence(user_id,task_id,request_id) ON DELETE CASCADE,
    FOREIGN KEY(user_id,assessment_id) REFERENCES learning_assessments(user_id,id) ON DELETE CASCADE,
    CHECK(body IS NULL OR (jsonb_typeof(body)='object' AND octet_length(body::text)<=12000)),
    CHECK((status='pending' AND body IS NOT NULL AND confirmation_request_id IS NULL AND assessment_id IS NULL AND expected_revision IS NULL AND confirmed_score IS NULL)
       OR (status='confirmed' AND body IS NOT NULL AND confirmation_request_id IS NOT NULL AND assessment_id IS NOT NULL AND expected_revision IS NOT NULL AND confirmed_score IS NOT NULL)
       OR (status='invalidated' AND body IS NULL AND confirmed_score IS NULL))
);
-- 来源清除时撤销由核验确认生成的自评，同时让旧快照确认失效。
CREATE FUNCTION invalidate_learning_review() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE changed BIGINT;
BEGIN
    UPDATE learning_assessments SET score=NULL
      WHERE user_id=OLD.user_id AND score IS NOT NULL
        AND id IN (SELECT assessment_id FROM learning_reviews WHERE user_id=OLD.user_id AND task_id=OLD.task_id);
    GET DIAGNOSTICS changed = ROW_COUNT;
    IF changed > 0 THEN
        UPDATE learning_state SET revision=revision+1 WHERE user_id=OLD.user_id;
    END IF;
    UPDATE learning_reviews SET status='invalidated',body=NULL,confirmed_score=NULL
      WHERE user_id=OLD.user_id AND task_id=OLD.task_id;
    RETURN OLD;
END;
$$;
CREATE TRIGGER learning_review_delete BEFORE DELETE ON learning_evidence
    FOR EACH ROW EXECUTE FUNCTION invalidate_learning_review();
CREATE TRIGGER learning_review_invalidate AFTER UPDATE OF deleted ON learning_evidence
    FOR EACH ROW WHEN (NEW.deleted) EXECUTE FUNCTION invalidate_learning_review();
