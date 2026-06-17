CREATE UNIQUE INDEX users_oidc_subject_uniq
    ON users (external_subject)
    WHERE provider = 'oidc' AND external_subject IS NOT NULL;
