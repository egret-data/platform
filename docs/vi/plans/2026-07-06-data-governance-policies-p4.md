# Kế hoạch triển khai — Data Governance Policies P4 (Ranger-parity core)

> Bản chuẩn (code đầy đủ): `docs/en/plans/2026-07-06-data-governance-policies-p4.md`. File này là tổng quan.

**Mục tiêu:** Thêm parity Ranger core: resource selector v2 (multi-value + include/exclude + recursive + autocomplete từ catalog metadata), Allow/Deny/Exceptions + deny-all-else, description + labels, inline enable toggle ở list, và fix summary union mọi bucket principal.

**Phụ thuộc P1–P3** (nhánh `feat/data-governance-policies`). Editor payload camelCase; list summary snake_case. KHÔNG `cargo build -p egret-controlplane` đơn lẻ.
**Component shadcn có sẵn:** `switch, tabs, textarea, accordion, checkbox, tooltip` — dùng thẳng.

## Danh sách task (TDD, commit từng task)

1. **Model + serializer** (`ranger_json.rs`): `AccessPolicyInput` thêm `allowExceptions`/`denyExceptions`/`isDenyAllElse`/`description`/`labels`; `access_to_ranger_json` emit chúng, deny-all-else ép rỗng deny buckets; Masking/RowFilter thêm description+labels. Round-trip test.
2. **Summary fix**: `summarize_policy` access union principals+permissions qua policyItems+denyPolicyItems+allowExceptions+denyExceptions (sửa bug deny-only); `PolicySummary` thêm description+labels. Test union.
3. **`set_policy_enabled` + `PUT /policies/{id}/enabled`** (`store.rs` + `mod.rs`): flip enabled + bump version + service policy_version. Route test.
4. **`GET /policies/resource-lookup`**: catalog từ `list_engine_catalogs` của engine gắn service + giá trị policy đã dùng; schema/table/column từ giá trị policy, lọc parent; `q` filter, cap 50. Route test.
5. **Frontend api**: types (exceptions/deny-all-else/description/labels; PolicySummary +description+labels) + `resourceLookup()` + `setPolicyEnabled()`; thêm field vào `blankInput` các dialog cho typecheck xanh.
6. **`ResourceLevelEditor.tsx`** (mới): Combobox creatable (Popover+Input+badge, gõ tự do + wildcard) gợi ý từ `resourceLookup`, multi-value + 2 Switch Exclude/Recursive, cascading theo parent. Vitest.
7. **Access editor** restructure: header (Name + Enabled + "…" popover Description/Labels) → 4 `ResourceLevelEditor` → Switch deny-all-else → **Allow|Deny Tabs** (ẩn Deny khi deny-all-else) + Accordion "Exceptions" mỗi tab (allow/denyExceptions). `toInput` đọc thêm field mới. Vitest (deny-all-else ẩn Deny, thêm allow exception).
8. **Masking/RowFilter + list**: đổi Input resource sang `ResourceLevelEditor`; thêm Description/Labels; `Policies.tsx` list thêm **inline Switch enable** (gọi `setPolicyEnabled` + invalidate) + cột Labels; `maskingToInput`/`rowfilterToInput` đọc description/labels + multi-value. Vitest.
9. **Full CI + smoke**: fmt/clippy/test workspace (DB) + frontend gate; smoke tạo policy multi-value + Exclude + exception + deny-all-else + toggle enable ở list + kiểm deny-only hiện principal.

## Ngoài P4 (P5+)
Roles, per-access isAllowed, validity, delegateAdmin, conditions, zones, policyPriority, import/export, grant/revoke, version history, live warehouse-browser enrichment cho schema/table/column.

## Điểm implementer xác nhận
Helper `req`/`session_for`/`into_json`; SQL bump `authz_services` khớp `update_policy`.
