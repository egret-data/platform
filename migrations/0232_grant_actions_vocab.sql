-- SP5a (governance full privilege vocabulary, concepts spec D4): widen the
-- grant action vocabulary from {read, write} to
-- {browse, read, write, create, manage}. A strict SUPERSET of 0154's CHECK,
-- so every existing row stays valid. OWNER is not an action (later task).
--
-- 0154 declared two inline, unnamed CHECKs on `actions` (cardinality and
-- vocabulary), so Postgres auto-named them; find the vocabulary one by its
-- definition rather than guessing the generated name.
DO $$
DECLARE c text;
BEGIN
    FOR c IN
        SELECT conname FROM pg_constraint
        WHERE conrelid = 'perm.grants'::regclass
          AND contype = 'c'
          AND pg_get_constraintdef(oid) LIKE '%<@%'
    LOOP
        EXECUTE format('ALTER TABLE perm.grants DROP CONSTRAINT %I', c);
    END LOOP;
END $$;

ALTER TABLE perm.grants ADD CONSTRAINT grants_actions_vocab
    CHECK (actions <@ ARRAY['browse', 'read', 'write', 'create', 'manage']);
