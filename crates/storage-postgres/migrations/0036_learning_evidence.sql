-- 每项训练最多保存一份不可变材料；删除保留请求墓碑，来源删除级联清除。
CREATE TABLE learning_evidence (
    user_id UUID NOT NULL,
    task_id UUID NOT NULL,
    request_id UUID NOT NULL CHECK(request_id<>'00000000-0000-0000-0000-000000000000'),
    body JSONB,
    created_ms BIGINT NOT NULL CHECK(created_ms>=0),
    deleted BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY(user_id,task_id),
    UNIQUE(user_id,request_id),
    FOREIGN KEY(user_id,task_id) REFERENCES learning_results(user_id,task_id) ON DELETE CASCADE,
    CHECK((deleted AND body IS NULL) OR (NOT deleted AND body IS NOT NULL)),
    CHECK(body IS NULL OR (jsonb_typeof(body)='object' AND octet_length(body::text)<=40000))
);
