# Data Governance — Policies P4 (Ranger-parity core) — Thiết kế

**Ngày:** 2026-07-06 · **Trạng thái:** Thiết kế — hướng đã duyệt · **Nhánh:** `feat/data-governance-policies` (tiếp P1–P3)

> Bản chuẩn: `docs/en/specs/2026-07-06-data-governance-policies-p4-design.md`.

## Mục tiêu
Đưa editor Policies của egret đạt parity chức năng với Ranger cho use-case Trino table-access: **resource selector** thật (multi-value + include/exclude + recursive + autocomplete từ catalog metadata), cấu trúc **Allow/Deny/Exceptions** + toggle **deny-all-else**, **description + labels**, **inline enable toggle** ở list, và **fix summary** phản ánh mọi bucket principal.

## Bối cảnh (nghiên cứu Ranger)
`RangerPolicy` mang nhiều hơn egret: allowExceptions/denyExceptions, isDenyAllElse, per-access isAllowed, roles, delegateAdmin, conditions, validitySchedules, description, policyLabels, policyPriority, zones… `RangerPolicyResource` có `values[]`/isExcludes/isRecursive (egret đã có field từ P2 nhưng UI chỉ nhập 1 giá trị/cấp, chưa lộ toggle). Service-def Trino **không** định nghĩa policyCondition nào → conditions là bloat cho Trino. UX Ranger đáng bê: deny-all-else thu gọn Deny, nhóm Allow/Deny+Exceptions, toggle exclude/recursive mỗi cấp, autocomplete resource, cột principal union mọi bucket. Ranger **thiếu** inline enable toggle ở list — egret nên làm tốt hơn.

## Quyết định scope (đã chốt)
- **VÀO P4 core:** resource selector v2 (multi-value + exclude/recursive + autocomplete); allowExceptions/denyExceptions + isDenyAllElse; description + labels; inline enable toggle ở list; fix `summarize_policy` union mọi bucket.
- **RA (P5/bỏ):** **roles** (hoãn — groups đã che phần lớn); **per-access isAllowed** (bỏ vì thừa với bucket Deny — YAGNI); validitySchedules, delegateAdmin, conditions, zones, policyPriority, import/export, grant/revoke, version history.
- **Nguồn autocomplete:** catalog từ `engine_catalogs` của engine gắn profile (metadata thật, DB); schema/table/column từ giá trị đã dùng trong policy của service (DB). Live warehouse-browser cấp dưới = stretch P5. Combobox luôn cho gõ tự do + wildcard.

## Backend
1. **Model/serializer** (`ranger_json.rs`): `AccessPolicyInput` thêm `allowExceptions`/`denyExceptions`/`isDenyAllElse`/`description`/`labels`. `access_to_ranger_json` emit các field này; deny-all-else → ép rỗng denyPolicyItems+denyExceptions. `ResourceLevel` đã có values/isExcludes/isRecursive (không đổi model). Masking/RowFilter thêm description+labels.
2. **Summary fix**: access union principals+permissions qua policyItems+denyPolicyItems+allowExceptions+denyExceptions (sửa bug deny-only trống). `PolicySummary` thêm description+labels.
3. **Endpoints**: `GET /policies/resource-lookup?service=&level=&catalog=&schema=&table=&q=` → string[] (catalog từ engine_catalogs của engine gắn service + giá trị policy đã dùng; schema/table/column từ giá trị policy đã dùng, lọc theo parent). `PUT /policies/{id}/enabled` {enabled} → `AuthzStore::set_policy_enabled` (flip enabled + bump version + service policy_version).

## Frontend
4. **`ResourceLevelEditor.tsx`** (mới, dùng chung): Combobox creatable (gõ tự do + wildcard) + gợi ý từ `resourceLookup`, multi-value (Badge xoá được) + 2 Switch Exclude/Recursive; cascading theo parent.
5. **Access editor** restructure: Header (Name + Enabled Switch + "…" popover Description/Labels) → Resources (4 ResourceLevelEditor) → Switch **deny-all-else** → **Allow|Deny Tabs**, mỗi tab có item rows + Accordion "Exceptions" thu gọn (allowExceptions/denyExceptions) → footer. `toInput` đọc thêm các field mới + multi-value.
6. **Masking/RowFilter + list**: thêm Description/Labels; đổi Input resource sang `ResourceLevelEditor`. `Policies.tsx`: **inline Switch enable** mỗi row (gọi `setPolicyEnabled` + invalidate), cột Labels, summary hiện đúng principal deny/exception. `api/policies.ts`: thêm types + `resourceLookup()` + `setPolicyEnabled()`.

## Kiểm thử
Unit: round-trip exceptions/deny-all-else/description/labels/multi-value qua summarize; summary union. Route(DB): create có exceptions+deny-all-else → list union; resource-lookup; PUT enabled; admin-gate. Vitest: ResourceLevelEditor, Access editor emit exceptions+isDenyAllElse, list enable Switch. Full CI + smoke.

## Phân pha trong P4
P4a model+serializer+summary+unit · P4b endpoints+route test · P4c ResourceLevelEditor+api · P4d Access editor+Masking/RowFilter+list · P4e CI+smoke.

## Rủi ro
Editor phình to (giữ Dialog/Sheet, Exceptions mặc định thu gọn); resource-lookup degrade về free-text; isDenyAllElse phải đồng bộ serialize↔UI; summary union phải dedupe; wire-format editor camelCase, list snake_case.
