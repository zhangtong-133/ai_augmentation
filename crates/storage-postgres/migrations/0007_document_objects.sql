-- 已有记录保留内联原文；新记录可引用私有对象桶。
ALTER TABLE documents
    ADD COLUMN original_object_key TEXT,
    DROP CONSTRAINT documents_original_format,
    ADD CONSTRAINT documents_original_format CHECK (
        (original_object_key IS NOT NULL AND length(original_object_key) > 0 AND original_pdf IS NULL AND original_html IS NULL) OR
        (original_object_key IS NULL AND (
            (source_type = 'markdown' AND original_pdf IS NULL AND original_html IS NULL) OR
            (source_type = 'pdf' AND original_pdf IS NOT NULL AND octet_length(original_pdf) BETWEEN 1 AND 5242880 AND original_html IS NULL) OR
            (source_type = 'web_page' AND original_pdf IS NULL AND original_html IS NOT NULL AND octet_length(original_html) BETWEEN 1 AND 1048576)
        ))
    );
CREATE UNIQUE INDEX documents_original_object_key_idx ON documents(original_object_key) WHERE original_object_key IS NOT NULL;
