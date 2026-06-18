CREATE TABLE authz_service_defs (
    id         bigint PRIMARY KEY,
    name       text NOT NULL UNIQUE,
    def_jsonb  jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE authz_services (
    id             bigserial PRIMARY KEY,
    name           text NOT NULL UNIQUE,
    type           text NOT NULL REFERENCES authz_service_defs(name),
    config_jsonb   jsonb NOT NULL DEFAULT '{}',
    policy_version bigint NOT NULL DEFAULT 1,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE authz_policies (
    id           bigserial PRIMARY KEY,
    service_id   bigint NOT NULL REFERENCES authz_services(id) ON DELETE CASCADE,
    name         text NOT NULL,
    policy_jsonb jsonb NOT NULL,
    enabled      boolean NOT NULL DEFAULT true,
    version      bigint NOT NULL DEFAULT 1,
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (service_id, name)
);

CREATE TABLE policy_audit (
    id        bigserial PRIMARY KEY,
    policy_id bigint,
    action    text NOT NULL,
    actor     uuid REFERENCES users(id),
    at        timestamptz NOT NULL DEFAULT now()
);
