-- Independent local consent shares the source lifecycle, never subscription identities.
ALTER TABLE learning_model_authorizations ADD COLUMN local_endpoint TEXT;
ALTER TABLE learning_model_authorizations ALTER COLUMN connection_id DROP NOT NULL;
ALTER TABLE learning_model_authorizations ALTER COLUMN connection_revision DROP NOT NULL;
ALTER TABLE learning_model_authorizations ADD CHECK (
 (local_endpoint IS NULL AND connection_id IS NOT NULL AND connection_revision IS NOT NULL)
 OR (local_endpoint IS NOT NULL AND connection_id IS NULL AND connection_revision IS NULL
     AND octet_length(local_endpoint) BETWEEN 1 AND 128)
);
