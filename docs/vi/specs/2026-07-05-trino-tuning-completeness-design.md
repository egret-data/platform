# Trino tuning completeness — guided config properties (2026-07-05)

Bản EN (canonical): `docs/en/specs/2026-07-05-trino-tuning-completeness-design.md`.

**Mục tiêu:** ~25 property Trino commonly-tuned có UI guided/typed/có help thay vì chỉ raw
`config.properties`, + search tìm property. Sub-project #2/4 (sau UX framework). **Chỉ frontend.**

**Kiến trúc then chốt:** gần như tất cả là config.properties key cluster-wide, và egret ĐÃ
render `configProperties` record vào config.properties trên CẢ 2 role (`config_properties(spec,coord)`).
Nên guided field chỉ đọc/ghi KEY đã biết trong map `configProperties` sẵn có — KHÔNG field
orchestrator mới, không carrier, không đổi shape payload (vẫn `trinoConfig.configProperties`).

**Non-goals:** field structured backend mới; security/TLS/access-control (#3); observability
(#4); key egret đã quản (query.max-memory=queryMaxMemory, log level, internal-communication.shared-secret auto) — loại khỏi catalog.

## Catalog
`config/knownProperties.ts` — `KnownProperty {key, section, label, type: string|number|boolean|enum|size|duration, default?, help, docsUrl?, enumOptions?}` + `knownProperties[]`. Curated ~25, mở rộng được. Type quyết định widget.

## Guided field — ghi vào configProperties
- `config/PropertyField.tsx` — render theo `prop.type`: size→QuantityInput; enum→Select; boolean→Switch; number/duration/string→Input; đều bọc FieldLabel (help/docs từ prop). Đọc/ghi 1 config.properties value.
- `useEngineConfigForm` thêm `getProp(key)` + `setProp(key,value)` trên state `configProps` (Kv[]) (upsert khi set, xoá khi rỗng). Không đổi payload.
- Raw config.properties KeyValueEditor (General/Advanced) LỌC bỏ key có trong knownProperties (guided UI sở hữu) → không trùng; vẫn xử lý key custom.

## UI — section nav mới
Thêm 4 domain section (sau Reliability, trước Advanced) + gộp 2 nhóm vào section có sẵn:
- **Query & Memory** (mới): max-total-memory, max-cpu-time, max-execution/run/planning-time, client.timeout, low-memory-killer.policy, max-history, max/min-hash-partition-count.
- **Spilling** (mới): spill-enabled, spiller-spill-path, max-spill-per-node, query-max-spill-per-node.
- **Execution & Optimizer** (mới): task.concurrency, task.max-worker-threads, join-distribution-type, join-reordering-strategy, node-scheduler.include-coordinator, max-splits-per-node, exchange.compression-codec.
- **SQL & Web UI** (mới): sql.default-catalog/schema, forced-session-time-zone, web-ui.enabled/authentication.type/shared-secret, catalog.management.
- **Reliability** (có sẵn): thêm query-retry-attempts + task-retry-attempts-per-task.
- **Advanced** (có sẵn): thêm log.format (TEXT/JSON).
Mỗi section render list knownProperties của nó qua PropertyField, bọc ConfigSection. `sectionCounts` thêm count cho section mới → badge chạy.

## Search
`config/PropertySearch.tsx` — ô search đầu Configuration card lọc knownProperties theo key/label/help; kết quả (key + section + help) click → chuyển nav sang section đó. Lo discoverability + long tail.

## Test
Unit (catalog→widget theo type; getProp/setProp upsert+remove; raw editor lọc known; built configProperties có key set, bỏ blank) + Playwright (section mới render + help; set spill-enabled + task.concurrency + join-distribution-type=BROADCAST → Save → PUT configProperties có các key đó; search "spill" nhảy tới Spilling) + live kind (configmap config-coordinator.properties có key).

Compat: frontend-only, không migration/API/payload-shape; raw config.properties vẫn dùng cho key custom; tái dùng primitive #1. Chi tiết xem bản EN.
