ALTER TABLE documents
    ADD COLUMN source_type TEXT NOT NULL DEFAULT 'markdown'
        CHECK (source_type IN ('markdown', 'pdf')),
    ADD COLUMN original_pdf BYTEA,
    ADD CONSTRAINT documents_pdf_original CHECK (
        (source_type = 'markdown' AND original_pdf IS NULL) OR
        (source_type = 'pdf' AND original_pdf IS NOT NULL AND octet_length(original_pdf) BETWEEN 1 AND 5242880)
    );
