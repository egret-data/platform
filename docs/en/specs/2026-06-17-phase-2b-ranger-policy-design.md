# Phase 2b — Apache Ranger Policy & REST Design Spec

> **Status:** Approved design · **Date:** 2026-06-17 · **Version:** 0.1
> Canonical (English). Vietnamese reference: [`docs/vi/specs/2026-06-17-phase-2b-ranger-policy-design.md`](../../vi/specs/2026-06-17-phase-2b-ranger-policy-design.md).
> Implements the **Apache Ranger** half of [Master Architecture](../architecture/2026-06-17-egret-architecture-design.md) §8.3 / Phase 2 (the data-plane catalog half shipped in Phase 2a). Validated against the `ranger` reference (`data-portal/ranger`).

---

## 1. Goal & scope

Expose a **Ranger Admin REST-compatible** surface so the native Apache Ranger plugins inside Trino (StarRocks later) download policies from Egret and enforce them at query time — access control, row-level filters, and column masking. No Ranger Admin is built or run; the engines use their stock plugin pointed at Egret.

**In scope**
- Two new crates: `egret-authz` (policy storage + versioning, owns the model and the seeded service-def) and `egret-ranger` (the Ranger REST-compatible HTTP surface + the Ranger JSON DTOs).
- **Full policy capability:** access (allow/deny per resource for user/group/role with access types), **row-level filters**, and **column masking** — i.e. Ranger's `policyItems`, `rowFilterPolicyItems`, and `dataMaskPolicyItems`.
- Ranger-compatible REST: service-def read, service CRUD, policy CRUD, and the **versioned policy download** (`lastKnownVersion` → 304/200) that plugins poll. Tag download is a benign empty stub.
- **Trino** engine: seed the official Trino Ranger service-def so the resource hierarchy and access types match exactly what the plugin/policy-engine expect.
- Tested against the **real Ranger policy engine** (`RangerPolicyEngineImpl` from `ranger-plugins-common`) loaded with Egret's `ServicePolicies` JSON, plus Rust integration (DB-gated) and unit tests.

**Out of scope (deferred)**
- StarRocks service-def (the framework is engine-agnostic; add its service-def later).
- Tag-based policies and a real `GET /service/tags/download/{service}` (returns an empty, well-formed payload this phase so plugins don't error).
- `egret-events` / `policy.changed` event bus — versioning is a DB counter the plugin polls.
- Admin UI policy authoring — backend only; UI arrives with the governance frontend phase.
- Ranger user/group/role sync REST, policy search, grant/revoke REST — not needed for download+enforce.
- Egret-internal enforcement of these policies (Egret's own query engine is Phase 3); this phase only serves policies the engine enforces.

## 2. Decisions

| # | Decision | Choice | Grounding |
|---|---|---|---|
| 1 | Spec shape | One spec: `egret-authz` + `egret-ranger` (tightly coupled — ranger serializes authz's stored policies) | Unlike 2a's catalog/ranger split, these are one deliverable |
| 2 | Policy capability | Access + row-filter + column-mask (all three Ranger policy types) | Full governance |
| 3 | Test bar | Real Ranger policy engine (`RangerPolicyEngineImpl`) evaluates Egret's `ServicePolicies` | The exact enforcement code the plugin runs |
| 4 | Engine | Trino first (seed official Trino service-def); StarRocks later | Most documented; one engine keeps scope tight |
| A | Storage shape | **Store native `RangerPolicy` JSON** (jsonb); download assembles `ServicePolicies`. No internal-model translation. | The whole purpose is Ranger compatibility; YAGNI vs §9.3's translate model |
| B | Versioning | **DB counter** `authz_services.policy_version`, bumped on policy change; download honors `lastKnownVersion` → 304. No `egret-events`. | Event bus not built; polling + counter suffices |
| C | Service-def | **Seed the official Trino service-def** from `ranger/agents-common/src/main/resources/service-defs/ranger-servicedef-trino.json` | Resource hierarchy + access types must match the plugin exactly |

## 3. Architecture / crate layout

```text
crates/
├── egret-authz/
│   └── src/
│       ├── lib.rs
│       ├── model.rs          # thin typed wrappers over RangerPolicy parts used internally
│       ├── store.rs          # AuthzStore (Postgres): service_defs, services, policies CRUD, version bump
│       ├── service_def.rs    # embedded Trino service-def (include_str!) + seed-on-init
│       ├── version.rs        # policy_version read/bump helpers
│       └── error.rs          # AuthzError
│
└── egret-ranger/
    └── src/
        ├── lib.rs
        ├── dto.rs            # RangerServiceDef, RangerService, RangerPolicy, ServicePolicies, ...
        │                     #   serde shapes matching ranger agents-common JSON byte-for-byte
        ├── api/
        │   ├── service_defs.rs  # GET definitions, GET definitions/name/{name}
        │   ├── services.rs      # services CRUD
        │   ├── policies.rs      # policies CRUD
        │   ├── download.rs      # policies/download/{service} (304/200), tags/download stub
        ├── error.rs          # RangerError → Ranger-style error body
        └── routes.rs         # ranger_router() mounted at /service
```

**Layering:** `ranger_router` (HTTP, Ranger JSON schema) → `egret-authz` (`AuthzStore` + versioning, Postgres). `egret-ranger` depends on `egret-authz`, `egret-metastore`, `egret-config`, `egret-core`. Mounted at **`/service`** (sibling of `/api` and `/catalog`) — engines call it directly, outside the browser session/CSRF stack.

## 4. REST API surface (mounted at `/service`)

Ranger Admin REST subset (base `/service/plugins`). Routes require the catalog/engine bearer token (§7).

| Method + path | Purpose |
|---|---|
| `GET /service/plugins/definitions` | All service-defs (seeded: trino). Returns `{ serviceDefs: [...] }`. |
| `GET /service/plugins/definitions/name/{name}` | One service-def by name. |
| `GET /service/plugins/services` · `POST` · `PUT /{id}` · `DELETE /{id}` | Service instance CRUD (a "service" = one Trino cluster). |
| `GET /service/plugins/policies` · `POST` · `PUT /{id}` · `DELETE /{id}` | Policy CRUD. Body is a Ranger `RangerPolicy`. Any write bumps the owning service's `policy_version`. |
| `GET /service/plugins/policies/download/{serviceName}?lastKnownVersion=N&pluginId=...` | **Plugin poll.** If `N == policy_version` → **304 Not Modified**; else **200** `ServicePolicies`. |
| `GET /service/tags/download/{serviceName}?lastKnownVersion=N` | Tag policies — returns a well-formed empty `ServiceTags` (or 304) this phase. |

Errors mirror Ranger's HTTP semantics (400 bad request, 404 not found, 409 on name conflict) with a JSON body; the exact Ranger `RESTResponse`/`VXResponse` shape is not required for the download path (plugins only parse success bodies + status codes), so a minimal `{ msg, statusCode }` body is sufficient.

## 5. Policy model & storage

**Store native (Decision A).** A `RangerPolicy` is stored as a `jsonb` column verbatim; the CRUD endpoints accept and return `RangerPolicy` JSON. The download endpoint assembles `ServicePolicies` from: the service row (`serviceName`, `serviceId`, `policyVersion`), the seeded `serviceDef`, and the service's enabled policies.

A `RangerPolicy` carries all three capability types (matching `ranger/.../model/RangerPolicy.java`):
- **`resources`**: map of resource-type → `{ values, isExcludes, isRecursive }` (e.g. `database`/`table`/`column` for Trino — exact keys come from the seeded service-def).
- **`policyItems[]`** (access): `{ accesses: [{type, isAllowed}], users, groups, roles, delegateAdmin, conditions }`.
- **`rowFilterPolicyItems[]`**: `policyItem` + `rowFilterInfo: { filterExpr }`.
- **`dataMaskPolicyItems[]`**: `policyItem` + `dataMaskInfo: { dataMaskType, valueExpr? }`.

`egret-authz::model` provides only thin typed helpers used internally (e.g. extracting the service name / id to bump version, validating the policy references a known service-def resource). It does NOT re-model the whole Ranger schema — the jsonb is the source of truth; `egret-ranger::dto` provides the serde types for the wire.

## 6. Download protocol & versioning (Decision B)

- `authz_services.policy_version` (bigint) starts at 1. Every successful policy create/update/delete on a service runs `policy_version = policy_version + 1` in the same transaction as the policy write.
- `GET .../policies/download/{serviceName}?lastKnownVersion=N`:
  1. Resolve the service by name → 404 if unknown.
  2. If `N == policy_version` → **304 Not Modified** (empty body).
  3. Else load the service's enabled policies, build `ServicePolicies { serviceName, serviceId, policyVersion, policyUpdateTime, serviceDef, policies }`, return **200**.
- The plugin caches the returned `policyVersion` and sends it as `lastKnownVersion` next poll — standard Ranger long-poll-free polling. No event bus; the plugin's poll interval (default ~30s) bounds propagation.

## 7. Auth & config

**Auth.** The `ranger_router` is gated by the same static bearer-token middleware pattern as the catalog (a `[ranger].auth_token`, or reuse the catalog token). The native Ranger plugin supports a Bearer/basic credential in its `policy.rest.url` config; we accept a bearer token. (Ranger's own plugin auth is typically Kerberos/basic; for Egret MVP a bearer token configured on the plugin side is the integration contract.)

**Config additions (`egret-config`).**
```toml
[ranger]
enabled    = false
auth_token = "..."   # static bearer (from env/secret); may equal catalog.auth_token
```
`validate()` rejects `ranger.enabled && auth_token.is_empty()`.

**Server wiring (`egret-server`).** `AppState` gains `ranger: Option<RangerState>`; `ranger_router()` is mounted at `/service` when `ranger.enabled`. The Trino service-def is seeded into `authz_service_defs` on startup (idempotent upsert by name) when ranger is enabled.

## 8. Data model (`migrations/0007_authz.sql`)

```sql
CREATE TABLE authz_service_defs (
    id         bigint PRIMARY KEY,          -- Ranger service-def id (trino seeded with a fixed id)
    name       text NOT NULL UNIQUE,        -- 'trino'
    def_jsonb  jsonb NOT NULL,              -- the full RangerServiceDef
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE authz_services (
    id             bigserial PRIMARY KEY,    -- Ranger service id
    name           text NOT NULL UNIQUE,     -- e.g. 'trino_prod'
    type           text NOT NULL REFERENCES authz_service_defs(name),  -- 'trino'
    config_jsonb   jsonb NOT NULL DEFAULT '{}',
    policy_version bigint NOT NULL DEFAULT 1,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE authz_policies (
    id           bigserial PRIMARY KEY,      -- Ranger policy id
    service_id   bigint NOT NULL REFERENCES authz_services(id) ON DELETE CASCADE,
    name         text NOT NULL,
    policy_jsonb jsonb NOT NULL,             -- native RangerPolicy
    enabled      boolean NOT NULL DEFAULT true,
    version      bigint NOT NULL DEFAULT 1,  -- per-policy version (Ranger tracks this too)
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (service_id, name)
);

CREATE TABLE policy_audit (
    id        bigserial PRIMARY KEY,
    policy_id bigint,                         -- nullable (service-level actions)
    action    text NOT NULL,                  -- create|update|delete
    actor     uuid REFERENCES users(id),
    at        timestamptz NOT NULL DEFAULT now()
);
```
Ranger ids are integers (not uuid) because the Ranger JSON model and plugin expect numeric `id`/`serviceId`/`policyId`. `bigserial` supplies them; the seeded service-def uses a fixed id.

## 9. Testing

- **Real Ranger policy-engine gate** (JVM, gated like the pyiceberg/Keycloak gates): a small Java harness (Maven/Gradle pulling `org.apache.ranger:ranger-plugins-common` from Maven Central) that (1) deserializes a `ServicePolicies` JSON produced by Egret's download endpoint into Ranger's `ServicePolicies`, (2) constructs a `RangerPolicyEngineImpl` with the Trino service-def, and (3) issues `RangerAccessRequest`s and asserts: an access policy allows/denies the expected `{catalog,schema,table,column}` + user/access-type; a row-filter policy yields the expected `filterExpr`; a data-mask policy yields the expected `maskType`. A Rust gate test shells out to the harness and self-skips unless `EGRET_TEST_RANGER_E2E=1` (needs a JVM + network for the jar). Runbook documented under `tests/ranger/README.md`.
- **Rust integration (DB-gated):** service-def seed/read; service + policy CRUD round-trip (POST a RangerPolicy → GET it back identical); `download` returns 200 with a well-formed `ServicePolicies` then 304 when `lastKnownVersion == policy_version`; a policy write bumps `policy_version`; `download` of an unknown service → 404.
- **Unit:** `ServicePolicies` assembly (service + def + policies); version-bump logic; the Trino service-def parses into `RangerServiceDef`; serde round-trip of a `RangerPolicy` with all three item types.

## 10. Decomposition for the plan

Suggested tasks: (1) scaffold `egret-authz` + `egret-ranger` crates + workspace deps; (2) `egret-ranger::dto` (RangerServiceDef/Service/Policy/ServicePolicies serde, validated against the seeded Trino def + a sample policy); (3) `migrations/0007_authz.sql` + `AuthzStore` service-def seed + read (DB-gated); (4) `AuthzStore` services CRUD + policies CRUD + `policy_version` bump (DB-gated); (5) `egret-authz::service_def` embed + idempotent seed-on-init; (6) `egret-ranger` download endpoint (304/200 + ServicePolicies assembly) + tags stub; (7) `egret-ranger` service/policy CRUD handlers + error mapping + bearer auth + `ranger_router`; (8) `egret-config` `[ranger]` + `egret-server` mount `/service` + seed-on-startup; (9) Rust integration tests (CRUD, download 200/304/404, version bump); (10) real Ranger policy-engine JVM gate + runbook; (11) final verification.

## 11. References

- **ranger** (`data-portal/ranger`): `agents-common/src/main/resources/service-defs/ranger-servicedef-trino.json` (seed), `agents-common/.../model/{RangerPolicy,RangerServiceDef,RangerService}.java` + `util/ServicePolicies.java` (wire shapes), `agents-common/.../policyengine/RangerPolicyEngineImpl.java` (test harness), `plugin-trino/` (the engine plugin that polls). Maven artifact `org.apache.ranger:ranger-plugins-common` for the test.
- Architecture: [Master design](../architecture/2026-06-17-egret-architecture-design.md) §8 (two-tier authz), §8.3 (Ranger REST), §9.3 (authz data model).
- Sibling: Phase 2a Iceberg catalog spec (same mount-outside-`/api` pattern, same bearer-auth approach).
