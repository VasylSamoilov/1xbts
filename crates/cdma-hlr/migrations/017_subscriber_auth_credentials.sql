-- A-key and SSD halves are 64-bit binary values.
ALTER TABLE subscriber_identities
    ADD COLUMN a_key BYTEA NULL,
    ADD COLUMN ssd_a BYTEA NULL,
    ADD COLUMN ssd_b BYTEA NULL,
    ADD COLUMN auth_count SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN auth_algo TEXT NOT NULL DEFAULT 'none',
    ADD COLUMN randssd BYTEA NULL,
    ADD COLUMN pending_rand BYTEA NULL,
    ADD COLUMN pending_randu BYTEA NULL,
    ADD COLUMN last_auth_at TIMESTAMPTZ NULL,
    ADD COLUMN auth_failure_count INTEGER NOT NULL DEFAULT 0;

ALTER TABLE subscriber_identities
    ADD CONSTRAINT chk_identity_a_key_len
        CHECK (a_key IS NULL OR octet_length(a_key) = 8),
    ADD CONSTRAINT chk_identity_ssd_a_len
        CHECK (ssd_a IS NULL OR octet_length(ssd_a) = 8),
    ADD CONSTRAINT chk_identity_ssd_b_len
        CHECK (ssd_b IS NULL OR octet_length(ssd_b) = 8),
    -- COUNT is the CAVE 6-bit call-history parameter (0..=63).
    ADD CONSTRAINT chk_identity_auth_count_range
        CHECK (auth_count >= 0 AND auth_count <= 63),
    ADD CONSTRAINT chk_identity_auth_failure_count_nonneg
        CHECK (auth_failure_count >= 0),
    ADD CONSTRAINT chk_identity_auth_algo
        CHECK (auth_algo IN ('none', 'cave'));
