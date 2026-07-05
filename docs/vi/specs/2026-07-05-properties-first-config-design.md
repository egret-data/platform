# Config UI properties-first (2026-07-05)

Bản EN (canonical): `docs/en/specs/2026-07-05-properties-first-config-design.md`.

**Mục tiêu:** Đưa Configuration UI về trọng tâm 1 **danh sách Properties** key/value tìm
kiếm được, backed by **toàn bộ catalog property Trino** — vì user thấy nhiều form structured
khó nhìn hơn editor "Config properties" đơn giản. Mọi giá trị là `config/node/log.properties`
gom vào 1 list Properties; chỉ config structured k8s/engine giữ form. Nav Configuration từ 14
→ ~5 section. **Chỉ frontend — không đổi payload** (vẫn ghi vào map configProperties/
additionalNodeProperties/additionalLogProperties sẵn có).

**Non-goals:** đổi backend; ép config structured (auth users, HPA/KEDA, pod, ingress,
resource-groups, catalogs) thành flat KV (form tốt hơn raw JSON cho các cái đó); #3/#4.

## Catalog Trino đầy đủ
`config/trinoCatalog.ts` — trích từ docs Trino (`trino/docs/src/main/sphinx/admin/properties-*.md`...):
`TrinoProp {key, group, file: "config"|"node"|"log", type: string|number|boolean|enum|size|duration,
default?, help, docsUrl?, enumOptions?}` + `trinoCatalog[]` (~200-400 entry — cả surface).
`file` = map .properties nào (gần hết `config`; vài `node`; per-logger `log`). `knownProperties.ts`
(31) được thay/gộp vào catalog này.

## Section Properties thống nhất
`config/PropertiesSection.tsx` — mặt chính:
- **Bảng property đang set** trên 3 map (configProperties/nodeProps/logProps): key + **ô value
  theo type** (`PropertyField` theo catalog `type` nếu key có trong catalog; input thường nếu custom)
  + remove. Chip group mỗi dòng.
- **Add property**: combobox trên TOÀN BỘ `trinoCatalog` (search key/label/help, group), hiện
  type/default/help; chọn → thêm dòng route theo `file` + value editor theo type; "custom key…"
  vẫn cho key tuỳ ý → config.properties.
- **Search box** lọc dòng + combobox.
- Đọc/ghi qua hook `configProps`/`nodeProps`/`logProps` (`getProp(file,key)`/`setProp(file,key,val)` —
  mở rộng getProp/setProp hiện có). Payload map không đổi.
Thay 4 tuning section (Query&Memory/Spilling/Execution/SQL&WebUI) + raw config/node/log editor.

## Nav restructure (14 → 5)
- **Properties** (list thống nhất; hút 4 tuning + raw config/node/log + queryMaxMemory/logLevel/log.format).
- **Resources** — per-role Coordinator/Worker (toggle): replicas/cpu/mem/heap/probes/jvm/
  query-max-memory-per-node/heap-headroom/graceful-shutdown + cluster env. (gộp Coordinator+Worker.)
- **Workload** — resource groups + session + fault-tolerant. (gộp Workload+Reliability.)
- **Scheduling** — Pod + Placement + Containers & Mounts (gộp 3 pod section; giữ collapsible con).
- **Scaling** — Autoscaling.
Security & Exposure giữ top-level tab; Catalogs ở Overview. ~5 section thì nav có thể phẳng (bỏ group
header) hoặc 2 group nhẹ — tuỳ implementer cho gọn. Badge non-default giữ.

## Xử lý component
Tái dùng: PropertyField/FieldLabel/QuantityInput/ConfigSection + editor role/pod/autoscaling/security/
resource-group (chuyển vào section gộp, nội bộ không đổi). Bỏ: QueryMemory/Spilling/Execution/SqlWebUi
Section (props giờ ở Properties list qua catalog); EngineConfigGeneral/Advanced co lại (raw KV → Properties,
env → Resources). PropertySearch (jump) được thay bằng search của Properties list — bỏ/repurpose. `sectionCounts`
đổi theo 5 key mới.

## Test
Unit (catalog integrity; getProp/setProp route theo file; Properties list gom key từ 3 map; add catalog prop
route đúng map + editor theo type; payload giữ shape 3 map) + Playwright (~5 nav; Properties + Add property
combobox full catalog; chọn spill-enabled(bool)/join-distribution-type(enum)/size prop; custom key; Save PUT
configProperties+node/log giữ shape) + live kind (configmap có key).

Compat: frontend-only, không migration/API/payload-shape (3 map property như cũ). Thay UI tuning-sections #2
(dùng cùng catalog data → PropertyField giữ; chỉ thay wrapper section). Chi tiết xem bản EN.
