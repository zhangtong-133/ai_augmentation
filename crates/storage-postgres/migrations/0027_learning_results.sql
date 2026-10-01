-- 训练结果独立于不可变计划；终结一次，不推断能力、不更新自评。
CREATE TABLE learning_results (
    user_id UUID NOT NULL,
    task_id UUID NOT NULL,
    request_id UUID NOT NULL CHECK(request_id<>'00000000-0000-0000-0000-000000000000'),
    outcome TEXT NOT NULL CHECK(outcome IN ('completed','cancelled')),
    note TEXT NOT NULL CHECK(char_length(note)<=2000 AND octet_length(note)<=8000),
    actual_minutes SMALLINT NOT NULL,
    recorded_ms BIGINT NOT NULL CHECK(recorded_ms>=0),
    PRIMARY KEY(user_id,task_id),
    UNIQUE(user_id,request_id),
    FOREIGN KEY(user_id,task_id) REFERENCES learning_tasks(user_id,id) ON DELETE CASCADE,
    CHECK((outcome='completed' AND actual_minutes BETWEEN 1 AND 180) OR (outcome='cancelled' AND actual_minutes=0))
);
CREATE FUNCTION clear_learning_result() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.status IN ('invalidated','deleted') THEN
        DELETE FROM learning_results WHERE user_id=NEW.user_id AND task_id=NEW.id;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER learning_result_cleanup AFTER UPDATE OF status ON learning_tasks
    FOR EACH ROW EXECUTE FUNCTION clear_learning_result();
