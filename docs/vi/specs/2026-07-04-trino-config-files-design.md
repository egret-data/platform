# Config-file Trino — resource groups, fault-tolerant, session/event, raw props (Phase 1.5.2, 2026-07-04)

Bản EN (canonical): `docs/en/specs/2026-07-04-trino-config-files-design.md`.
Nối tiếp Phase 1.5.1 (#139).

**Mục tiêu:** Thêm nhóm config-file (config nằm ở file/JSON riêng, không phải key
config.properties):
1. **Resource groups** — queuing/concurrency (structured builder)
2. **Fault-tolerant execution** — `retry-policy` + exchange manager S3 (tái dùng storage connection egret)
3. **Session properties** — structured builder
4. **Event listener** — raw `event-listener.properties`
5. **Raw node.properties / log.properties**
6. **`memory.heap-headroom-per-node`** (per-role)

**Non-goals:** autoscaling (1.5.4), pod/scheduling (1.5.3), TLS/auth+networking (Phase 1.6), observability (1.5.5).

## Data model — thêm vào TrinoConfig
- Per-role: `heapHeadroomPerNode?`
- Cluster: `additionalNodeProperties`, `additionalLogProperties`, `eventListener` (map raw);
  `resourceGroups?{rootGroups:Group[], selectors:Selector[]}` (Group đệ quy: name/softMemoryLimit/
  hardConcurrencyLimit/maxQueued/schedulingPolicy?/subGroups[]; Selector{user?,source?,queryType?,group});
  `sessionProperties?: Rule[]` (Rule{matchUser?,matchSource?,matchClientTags?[],properties});
  `faultTolerant?{retryPolicy:"TASK"|"QUERY", exchangeStorageConnId, exchangePrefix}`.

## Orchestrator (trino.rs) — chart-verified
- resourceGroups → `resource-groups.json` + `resource-groups.properties` (coordinator)
- session → `session-property-config.json` + `.properties`
- event → `event-listener.properties` + wire `event-listener.config-files`
- fault-tolerant → cả 2 role: `retry-policy` trong config.properties + `exchange-manager.properties`
  (`name=filesystem`, `exchange.base-directories=s3://<bucket>/<prefix>`, `exchange.s3.*` + creds `${ENV:...}`)
- node/log raw append · heapHeadroom per-role. File mới subPath-mount /etc/trino/...

## Control-plane
- Carrier + serialize JSON structured + **validate** (selector trỏ group tồn tại) → 400 nếu sai.
- **Fault-tolerant**: resolve `exchangeStorageConnId` (bucket/endpoint/region + creds qua resolve_plaintext)
  giống egret-warehouse catalog → thread S3 coords + creds vào env Secret. base dir = `s3://{bucket}/{prefix}`.

## Frontend (EngineDetail Cluster tab)
- Fault-tolerant card (toggle + retry-policy + storage `<Select>` + prefix)
- Resource groups builder (cây group đệ quy + selectors) · Session builder (rule list) ·
  event-listener + additional node/log KV · per-role thêm heapHeadroomPerNode.

## Test
Unit (render resource-groups.json/session.json/exchange-manager.properties/retry-policy/heapHeadroom/raw) +
live kind (bật fault-tolerant + 1 resource group → roll → verify file trong pod + query chạy).

Compat: field optional, không migration; engine cũ không đổi.

Chi tiết đầy đủ xem bản EN.
