ALTER TABLE documents
    DROP CONSTRAINT documents_source_type_check,
    DROP CONSTRAINT documents_pdf_original,
    ADD COLUMN original_html TEXT,
    ADD CONSTRAINT documents_source_type_check CHECK (source_type IN ('markdown', 'pdf', 'web_page')),
    ADD CONSTRAINT documents_original_format CHECK (
        (source_type = 'markdown' AND original_pdf IS NULL AND original_html IS NULL) OR
        (source_type = 'pdf' AND original_pdf IS NOT NULL AND octet_length(original_pdf) BETWEEN 1 AND 5242880 AND original_html IS NULL) OR
        (source_type = 'web_page' AND original_pdf IS NULL AND original_html IS NOT NULL AND octet_length(original_html) BETWEEN 1 AND 1048576)
    );
