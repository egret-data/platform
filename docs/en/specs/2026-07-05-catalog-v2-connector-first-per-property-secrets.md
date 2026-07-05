# Engine Catalogs v2 — Connector-First + Per-Property Secrets — Design

- **Status:** Approved (design)
- **Date:** 2026-07-05
- **Branch:** `feat/engine-config-ui-remap`
- **Supersedes parts of:** `docs/en/specs/2026-07-05-catalog-view-edit-secrets-design.md` (the "generic connector" wrapper + one-secret-per-catalog model)

## 1. Motivation (review feedback on v1)

1. v1 buried every connector under a single **"Generic connector"** kind (with PostgreSQL also duplicated as its own kind). Connectors should be **first-class**.
2. v1 allowed **one secret per catalog** mapped to one designated property. A secret fills a property **value**; the user should pick **secret-or-plaintext per property**, and a catalog may use several secrets.
3. v1 enumerated only ~9 connectors. Support should be **comprehensive** (+ custom for the long tail).

**Decision:** *uniform connectors* — one connector-first picker for everything; PostgreSQL loses its bespoke host/port/db/user form and uses the same property form; the "generic" concept is dropped from the UI.

## 2. Design

### 2.1 Connector model (storage + API)

- A catalog's `kind` **is the Trino connector name** (`mysql`, `postgresql`, `iceberg`, `hive`, `tpch`, …). `egret-warehouse` remains a special kind (Iceberg→egret).
- `config` for a connector catalog:
  ```jsonc
  {
    "properties":       { "<prop>": "<plaintext value>", … },
    "secretProperties": { "<prop>": "<egret-secret-uuid>", … }
  }
  ```
  `properties` and `secretProperties` have **disjoint keys** (a property is either typed or secret-backed). No `connectorName`/`secretProperty` (v1) — the connector is the `kind`.
- `add_catalog` accepts `{name, kind, config}`; `update_catalog` `{config}`. The `CATALOG_KINDS` whitelist is **relaxed** — any non-empty connector name is allowed (plus `egret-warehouse`). The legacy DB `secret_ref` column is unused by v2 rows (kept for back-compat; not written).

### 2.2 Backend `build_catalog_req`

- **egret-warehouse:** unchanged (special Iceberg→egret credential-via-env path).
- **Any other kind (uniform connector arm):**
  - `props["connector.name"] = cat.kind`.
  - Emit every `config.properties` entry verbatim.
  - For each `(key, secretUuid)` in `config.secretProperties`: resolve the secret (`st.secrets.resolve_plaintext(None, uuid)`) into a **distinct** env var `CAT_{env_prefix}_{n}` (n = stable index) and set `props[key] = "${ENV:CAT_{env_prefix}_{n}}"`. Multiple secrets per catalog.
- **Back-compat shim** (bounded) for rows written by v1/older: if `config` has no `properties` key but has legacy shapes — `connectorName`+`properties`+`secretProperty`+`secret_ref` (old generic) or `host`/`port`/`database`/`user`+`secret_ref` (old postgres) — map them as v1 did. New rows always use the v2 shape. tpch/tpcds: empty `config` → uniform arm emits just `connector.name`, correct.

### 2.3 Frontend — connector-first picker

- The catalog **Kind** control becomes a searchable **Connector** picker listing the full `connectorCatalog` (see 2.5) + **Egret warehouse** (special) + **Other (custom)** (free-typed connector name, empty property catalog). No "generic" entry, no separate PostgreSQL entry.
- The stored `kind` = the chosen connector's `name` (or the custom string).

### 2.4 Frontend — per-property secret-or-value editor

- Replace the plain `KeyedPropertyList` value column (for catalogs) with a **`SecretOrValueList`**: each property row renders its typed value editor **plus** a toggle ("Use secret" / key icon). Toggled → the value field becomes an egret-secret `Select` (from `listSecrets()`); untoggled → the normal `PropertyField`/input.
- The list is scoped to the selected connector's property catalog (picker suggests known props; custom keys allowed), same as v1's connector `KeyedPropertyList`.
- On submit, split rows into `properties` (typed) and `secretProperties` (secret-backed → the secret's uuid). Edit prefill: `properties` → typed rows; `secretProperties` → secret-toggled rows (the Select shows the referenced secret).

### 2.5 Comprehensive connector catalog

Expand `connectorCatalog.ts` to the common Trino connectors, each `{name, label, props: TrinoProp[]}` (curated real props where well-known; empty ⇒ custom-only):
- **Lakehouse:** iceberg, hive, delta_lake, hudi.
- **JDBC:** postgresql, mysql, mariadb, sqlserver, oracle, redshift, clickhouse, singlestore, snowflake, druid, ignite, phoenix.
- **NoSQL / search / stream:** mongodb, cassandra, elasticsearch, opensearch, redis, kafka, kinesis, prometheus.
- **Datasets / system:** tpch, tpcds, blackhole, memory, jmx, system, googlesheets, bigquery.
Curate high-value props for the top connectors (iceberg/hive/delta/postgresql/mysql/sqlserver/clickhouse/kafka/mongodb — reuse v1's) and label-only (empty props) for the rest; custom keys always addable.

## 3. Component work (reworks v1)

- **Backend:** `build_catalog_req` uniform arm + per-property secrets + compat shim (`engines.rs`); relax `CATALOG_KINDS`; `add_catalog`/`update_catalog` accept connector kinds; redaction unchanged (config holds uuids, not plaintext — but keep egret-warehouse redaction).
- **Frontend:** expand `connectorCatalog.ts`; new `SecretOrValueList.tsx`; rework `CatalogForm.tsx` to the uniform model (drop postgres structured form + generic wrapper + v1 single-secret block); `catalogKinds.ts` retired/replaced by the connector picker.

## 4. Non-goals
- Not migrating existing stored catalog rows (the compat shim renders them; they can be re-saved into the v2 shape).
- egret-warehouse stays special/read-only.
- No DB migration (config JSON shape change only; `secret_ref` column left in place, unused by v2).

## 5. Risks
- **Secret leakage:** `config.secretProperties` stores secret **uuids** (references), never plaintext — safe to return from the detail endpoint (uuids were already exposed via `secretRef` in v1). egret-warehouse `credential` redaction stays.
- **Env-var collision:** per-secret env vars must be unique per catalog (`CAT_{prefix}_{n}` with a stable index); verify two secret props don't collide.
- **Back-compat:** the shim must render old postgres/generic rows unchanged (test both old shapes).
- **Parity:** a connector catalog with only plaintext props (no secrets) renders exactly like v1's generic path did (connector.name + verbatim props).

## 6. Acceptance
- Kind picker lists connectors directly (Iceberg/Hive/Postgres/MySQL/… + Egret warehouse + Other); no "generic".
- A property value can be toggled to a secret; a catalog can have ≥2 secret-backed properties, each rendering `${ENV:…}` with its own env entry.
- Editing prefills typed + secret-backed properties correctly; egret-warehouse read-only.
- Old-shaped rows still provision (compat shim). New rows use `{properties, secretProperties}`.
- Backend fmt/clippy/test + frontend lint/typecheck/test/build green; live smoke on `demo2`.
