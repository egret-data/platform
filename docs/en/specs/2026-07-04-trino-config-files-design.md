# Trino config-file surface — resource groups, fault-tolerant execution, session/event, raw props (Phase 1.5.2, 2026-07-04)

VI reference: `docs/vi/specs/2026-07-04-trino-config-files-design.md`.
Builds on Phase 1.5.1: `docs/en/specs/2026-07-04-trino-config-expansion-design.md` (shipped, #139).

**Goal:** Add the tier-ORANGE Trino config-file surface — the config that lives in
dedicated files/JSON rather than plain `config.properties` keys:
1. **Resource groups** — query queuing / concurrency limits (structured builder).
2. **Fault-tolerant execution** — `retry-policy` + an S3 exchange manager for
   spooling (reuses an egret storage connection).
3. **Session properties** — session-property manager rules (structured builder).
4. **Event listener** — raw `event-listener.properties`.
5. **Raw `node.properties` / `log.properties`** passthroughs.
6. **`memory.heap-headroom-per-node`** (per-role).

**Non-goals:** autoscaling (1.5.4), scheduling/pod-spec (1.5.3), TLS/auth +
networking (Phase 1.6 with the proxy), observability (1.5.5). GC method stays
hardcoded.

## Chart-verified rendering targets
- Resource groups: `resource-groups.properties` (`resource-groups.configuration-manager=file` + `resource-groups.config-file=/etc/trino/resource-groups/resource-groups.json`) + a `resource-groups.json`. Coordinator-only.
- Session: `session-property-config.properties` (`configuration-manager=file` + `session-property-manager.config-file=/etc/trino/session-property-config.json`) + a `session-property-config.json`. Coordinator-only.
- Event listener: `event-listener.properties` + `event-listener.config-files` wiring in config.properties.
- Fault-tolerant: `retry-policy=<policy>` in config.properties (both roles) + `exchange-manager.properties` (both roles) with `exchange-manager.name=filesystem`, `exchange.base-directories=s3://<bucket>/<prefix>`, `exchange.s3.region/endpoint/path-style-access`, `exchange.s3.aws-access-key/aws-secret-key=${ENV:...}`.

## Data model — `TrinoConfig` additions

Per-role (`RoleConfig`): `heapHeadroomPerNode?: string` → `memory.heap-headroom-per-node`.

Cluster-wide (`TrinoConfig`):

```
additionalNodeProperties: Record<string,string>   // appended to node.properties
additionalLogProperties: Record<string,string>    // appended to log.properties
eventListener: Record<string,string>               // event-listener.properties (raw)
resourceGroups?: {
  rootGroups: Group[]
  selectors: Selector[]
}
sessionProperties?: SessionRule[]
faultTolerant?: {
  retryPolicy: "TASK" | "QUERY"
  exchangeStorageConnId: string   // an egret storage connection (the spool bucket)
  exchangePrefix: string          // key prefix under that bucket
}
```

```
Group {
  name: string
  softMemoryLimit: string          // "80%" or "10GB"
  hardConcurrencyLimit: number
  maxQueued: number
  schedulingPolicy?: string        // e.g. "weighted_fair" (optional)
  subGroups: Group[]               // recursive
}
Selector { user?: string, source?: string, queryType?: string, group: string }
SessionRule { matchUser?: string, matchSource?: string, matchClientTags?: string[], properties: Record<string,string> }
```

Recursive `subGroups` match the chart's resource-groups JSON exactly. Selectors use
a dot-path `group` string (e.g. `global.etl`) referencing the group hierarchy.

## Orchestrator rendering (`trino.rs`)

Extend `trino_config_map` (coordinator/worker as noted):
- **`resource-groups.json`** (coordinator) — serialise `{rootGroups, selectors}` to
  the Trino resource-groups JSON (rootGroups keep their nested `subGroups`; selectors
  become `[{user?, source?, queryType?, group}]`). Add `resource-groups.properties`
  to the config map + a `/etc/trino/resource-groups/resource-groups.json` subPath mount.
- **`session-property-config.json`** (coordinator) — serialise the rules to
  `{sessionProperties:[{matchUser?→user, matchSource?→source, clientTags?, sessionProperties}]}`
  + `session-property-config.properties`.
- **`event-listener.properties`** — the raw `eventListener` map; wire
  `event-listener.config-files=/etc/trino/event-listener.properties` in config.properties.
- **`config_properties(role)`** additions: `memory.heap-headroom-per-node` (per-role
  from `heapHeadroomPerNode`); when `faultTolerant` set, `retry-policy=<policy>` (both roles).
- **`exchange-manager.properties`** (both roles when `faultTolerant` set) — from the
  resolved exchange S3 coordinates (see control-plane). Add to the config map + mount.
- **node.properties** / **log.properties** — append `additionalNodeProperties` /
  `additionalLogProperties`.
- All new files are subPath-mounted under `/etc/trino/...`; the config-checksum
  annotation already rolls the pods on change.

## Control-plane

- Carriers gain the new fields (kube-free mirror). Structured `resourceGroups` /
  `sessionProperties` are carried as their JSON and **validated** (well-formed +
  group names referenced by selectors exist) before apply; a bad config → 400.
- **Fault-tolerant storage wiring**: `exchangeStorageConnId` names an egret storage
  connection. The handler resolves it (bucket, endpoint, region, path-style) + its
  secret (access/secret key via `resolve_plaintext`) exactly like the egret-warehouse
  catalog does, and threads the S3 coordinates + creds into the provisioning request
  so the orchestrator renders `exchange-manager.properties` with the creds referenced
  via env (`${ENV:EXCHANGE_*}`) sourced from the engine env Secret. 400 if the
  connection or its secret is missing.
- The exchange base directory is `s3://{bucket}/{exchangePrefix}`.

## Frontend (EngineDetail)

Extend the **Cluster** tab (and per-role tabs for heap headroom):
- **Fault-tolerant execution** card: a toggle → retry-policy `<Select>` (TASK/QUERY),
  an exchange-storage `<Select>` (egret storage connections via `listStorage`), a
  prefix input.
- **Resource groups** builder: a recursive group tree editor (add/remove group, its
  softMemoryLimit / hardConcurrencyLimit / maxQueued / schedulingPolicy, nested
  subgroups) + a selectors list (user/source/queryType → group).
- **Session properties** builder: a list of rules (match user/source/clientTags →
  a properties KV editor).
- **Event listener** + **additional node/log properties**: raw `KeyValueEditor`s.
- Per-role tabs: a `heapHeadroomPerNode` input next to the existing per-node query
  memory field.

## Test

- **Unit** (fake kube): `resource-groups.json` + `resource-groups.properties`
  serialise from a 2-level group tree + selectors; `session-property-config.json`;
  `event-listener.properties`; `retry-policy` in both roles' config; `exchange-manager.properties`
  with `s3://bucket/prefix` + `${ENV:...}` creds; per-role `memory.heap-headroom-per-node`;
  appended node/log properties.
- **Live (kind)**: enable fault-tolerant pointing at the demo storage connection +
  add one resource group with a selector → roll → `kubectl` (`-o yaml`, ANSI-stripped)
  shows `resource-groups.json`, `exchange-manager.properties` (s3 base dir + creds),
  `retry-policy=TASK`; run a query to confirm the cluster still serves.

## Compat
- All new fields are optional; existing engines are unaffected (nothing renders when
  a field is unset). No DB migration (spec jsonb grows in place).
