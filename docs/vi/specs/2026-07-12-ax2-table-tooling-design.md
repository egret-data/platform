# AX-2 — Table Tooling + Density — Thiết kế

**Trạng thái:** Đã duyệt
**Ngày:** 2026-07-12
**Sáng kiến:** Access-Control UX uplift (increment 3/5) —
`docs/en/plans/2026-07-12-access-control-ux-uplift-roadmap.md`

> Bản tiếng Việt là tham chiếu; bản chuẩn là
> `docs/en/specs/2026-07-12-ax2-table-tooling-design.md`.

## Mục tiêu

Làm mọi danh sách Access-Control dễ quét/tìm và đạt đúng "data-dense" như phong cách
tự nhận. Thêm search + faceted filter + sort cột cho bảng Users/Groups/Roles, thêm ô
search cho catalog Permissions, và thay 3 KPI card to (`StatCardRow` — số bé, card to,
trống ~40% trang) bằng **dải summary 1 dòng** gọn. Thuần frontend — không đổi backend.

## Hiện trạng

- **Users / Groups / Roles**: mỗi trang có `StatCardRow` (3 KPI card to) + raw `<Table>`
  **không search/filter/sort**. Anti-pattern design-system là "no filtering"; ở quy mô lớn
  không dùng nổi, và KPI card to đọc như filler.
- **Permissions**: lưới 2 cột card gom nhóm (đã là chuẩn dense — không KPI) nhưng **không
  search** ~40 permission.
- **FilterableTable** v1 (làm ở AX-1) đã có search + sort + slot `filters?: ReactNode` trên
  API `Column<T> = { key, label, sortable?, className?, value?, cell }`. AX-2 tổng quát hoá
  + thêm control facet chuẩn.

## Thiết kế

### 1. `FacetSelect` — control filter tái dùng
Thêm `FacetSelect` (đặt cạnh `FilterableTable`, vd `frontend/src/components/explore/FacetSelect.tsx`)
— 1 shadcn `Select` có label cho 1 facet:
```
FacetSelect({ label, value, onChange, options }: {
  label: string;
  value: string;                       // "" | "__all__" = không lọc
  onChange: (v: string) => void;
  options: { value; label }[];         // không gồm sentinel "All" (component tự prepend)
})
```
**Lọc nằm ở page, không trong FilterableTable.** Mỗi page giữ state facet, lọc mảng `rows`
(facet + predicate riêng), truyền rows đã-lọc vào `FilterableTable` (nó chỉ search+sort
lên trên). Giữ FilterableTable đơn nhiệm; logic domain filter ở chỗ domain data. Các
`FacetSelect` render vào slot `filters` sẵn có.

### 2. `SummaryStrip` — đếm gọn
Thêm helper `SummaryStrip` (đặt cạnh): 1 dòng `text-sm text-muted-foreground` gồm các cặp
`label: value` (value `font-medium text-foreground tabular-nums`), cách nhau bằng middot —
mirror dải summary của trang Members (AX-1). Thay các khối `StatCardRow`.

### 3. Áp từng trang

| Trang | Summary strip | Search | Facet filters | Cột sort |
|---|---|---|---|---|
| **Users** | "N users · N active · N disabled" | email/name | **Status** (active/disabled) · **Provider** (local/sso) | Email, Name |
| **Groups** | "N groups" (+ số member nếu có sẵn rẻ) | tên group | — (nếu có `source` thì Source local/oidc) | Name |
| **Roles** | "N roles · N platform · N workspace" | tên/description | **Scope** (platform/workspace) | Name |
| **Permissions** | *(giữ nguyên — không KPI)* | key/description (lọc card) | *(không — giữ lưới card; chỉ thêm search)* | *(giữ nguyên)* |

- **Users / Groups / Roles**: thay `StatCardRow` bằng `SummaryStrip`; thay raw `<Table>`
  bằng `<FilterableTable>` (map cột hiện có sang `Column<T>`, **giữ nguyên mọi cell
  renderer** — badge, chip roles/groups, menu `⋯`, drill-in assignee…); thêm `FacetSelect`
  vào slot `filters` + áp trong bộ lọc rows của page. **Giữ mọi hành vi cũ** (dialog
  create/edit/assign, delete, deep-link "View access" từ AX-3, hiển thị last-admin guard).
- **Permissions**: giữ lưới 2 cột card gom nhóm; thêm 1 `Input` search đầu trang lọc
  permission nào render (match `key`/`description`, không phân biệt hoa thường); group mà
  mọi permission bị lọc hết thì ẩn. Không summary strip, không facet, không đổi layout khác.

### 4. Nhất quán
`StatCard`/`StatCardRow` **giữ trong codebase** (dùng chỗ khác — Engines/Warehouses control-plane);
AX-2 chỉ NGỪNG dùng chúng trên 4 trang access. Không xoá.

## Kiểm thử

**Frontend (Vitest)** — mỗi trang:
- **Users**: gõ search thu hẹp rows; facet Status lọc active/disabled; facet Provider lọc
  local/sso; summary strip số đúng; sort cột đảo thứ tự. Hành vi cũ (create/edit, "View
  access") còn chạy.
- **Groups**: search theo tên; summary count đúng.
- **Roles**: search thu hẹp; facet Scope lọc platform/workspace; summary total/platform/
  workspace; badge built-in + drill-in assignee còn render.
- **Permissions**: ô search lọc permission xuyên group; group không match bị ẩn; xoá search
  khôi phục hết.
- **`FacetSelect`**: render option "All" + option truyền vào; chọn gọi `onChange`.

## Ngoài phạm vi
- **AX-4** — cây hierarchy trên Roles, role-editor scope-filter, scope clarifier.
- **AX-5** — bulk ops + undo.
- Convert Permissions sang FilterableTable phẳng (lưới card là chuẩn dense — giữ).
- Bất kỳ đổi backend nào (thuần trình bày).

## Ghi chú
- Tái dùng: `FilterableTable` + `Column<T>`/slot `filters` (AX-1), shadcn `Select`/`Input`,
  dải summary trang Members (AX-1) làm tham chiếu thị giác.
- Giữ diff mỗi trang gọn: đổi container (StatCardRow→SummaryStrip, Table→FilterableTable) +
  thêm state filter; không tái cấu trúc dialog/data hooks.
