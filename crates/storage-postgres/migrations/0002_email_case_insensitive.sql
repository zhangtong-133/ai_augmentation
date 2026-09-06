-- If existing data contains case-only duplicates, fail instead of deleting data.
CREATE UNIQUE INDEX users_email_lower_idx ON users (lower(email));
