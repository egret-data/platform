# Gom nav config + combobox key property (2026-07-05)

Bản EN (canonical): `docs/en/specs/2026-07-05-config-nav-grouping-property-combobox-design.md`.

**Mục tiêu:** 2 refinement UX theo feedback: (1) nav Configuration 14 item phẳng rối → gom
vào vài nhóm collapsible; (2) ô key raw `config.properties` (Add property) đang gõ tự do →
đổi thành combobox gợi ý tên property Trino (typeahead) + vẫn cho gõ custom. **Chỉ frontend.**

**Non-goals:** field config mới; đổi hành vi field; #3/#4. Tái dùng primitive #1.

## 1. Nav Configuration gom nhóm collapsible
Thay nav 14-item phẳng (trong `ConfigurationTab`) bằng danh sách gom nhóm — 5 nhóm collapsible,
mỗi nhóm 1 header (chevron toggle) + section con:
- **Basics**: General
- **Roles**: Coordinator, Worker
- **Workload & Tuning**: Workload, Reliability, Query & Memory, Spilling, Execution & Optimizer, SQL & Web UI
- **Pods & Scheduling**: Pod, Placement, Containers & Mounts, Autoscaling
- **Advanced**: Advanced

Hành vi: mảng `groups {label, sections[]}`; state mở/đóng từng nhóm; load: mở nhóm chứa section
đang active (+ Basics), đóng còn lại. Chọn section vẫn set active. Badge non-default giữ ở item;
nhóm đóng hiện badge tổng (tránh giấu config đã set). Dùng `Collapsible` sẵn có. KHÔNG bỏ section
nào — chỉ tổ chức lại.

## 2. Combobox key trong raw config.properties editor
- **`config/trinoPropertyNames.ts`** — catalog rộng `{key, help?}[]` ~100+ tên config.properties
  Trino (từ docs), superset của 31 guided.
- **`KeyValueEditor`** thêm prop optional `keySuggestions?: {key,help?}[]`. Khi có → ô key thành
  combobox: gõ → dropdown ~8 gợi ý khớp (key + help); click điền key; gõ custom vẫn nhận (Enter/blur).
  Không truyền prop → editor như cũ (env/annotations không đổi).
- config.properties editor (`EngineConfigGeneral`) truyền `keySuggestions =
  trinoPropertyNames.filter(!knownPropertyKeys.has(key))` — gợi ý LONG-TAIL (31 guided ở section
  riêng). `onChange` DROP row có key thuộc `knownPropertyKeys` (`newRows.filter(!known)`) → guided
  key gõ vào raw không persist thành dòng trùng/ma (fix "query.max-run-time × 3": key đó guided ở Query & Memory).

## Test
Unit (keySuggestions loại guided; raw onChange drop known; groups phủ đủ 14 section) + Playwright
(nav ~5 group header; thu/mở nhóm ẩn/hiện item; nhóm đóng có config hiện badge tổng; Add property gõ
"spill" gợi ý long-tail key rồi chọn; custom key vẫn gõ được; guided key không suggest + gõ ép không
persist; Save PUT payload không đổi).

Compat: frontend-only, không migration/API/payload; gom nav thuần visual; combobox additive (KeyValueEditor
không đổi khi không có keySuggestions). Chi tiết xem bản EN.
