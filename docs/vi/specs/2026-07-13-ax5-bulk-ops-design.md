# AX-5 — Bulk Ops + Safety Polish — Thiết kế

**Trạng thái:** Đã duyệt
**Ngày:** 2026-07-13
**Sáng kiến:** Access-Control UX uplift (increment 5/5 — cuối cùng) —
`docs/en/plans/2026-07-12-access-control-ux-uplift-roadmap.md`

> Bản tiếng Việt là tham chiếu; bản chuẩn là
> `docs/en/specs/2026-07-13-ax5-bulk-ops-design.md`.

## Mục tiêu

Cho admin thao tác nhiều hàng cùng lúc, an toàn: multi-select trên bảng Users và Workspace
Members với bulk actions (disable/enable users, gán role cho nhiều, remove members, đổi
role nhiều members), mỗi action reversible có **Undo**. Hoàn thiện polish: badge state
"active" dùng màu semantic tách khỏi brand, thay text "Loading…" tùy tiện bằng skeleton
nhất quán. **FE-only** — bulk loop endpoint single sẵn có; không đổi backend.

## Hiện trạng

- Bảng access (`FilterableTable`, AX-2) chưa có selection — mọi action từng-hàng-một.
- `useBulkSelect<T extends {id}>(rows)` đã có → `{bulkMode, setBulkMode, selected,
  selectedIds, toggle, toggleAll, clear, allSelected, someSelected}`; `ListSkeleton` đã có
  — cả hai từ BI lists. FilterableTable chỉ cần optional selection.
- Endpoint single đã có: `updateUser(id,{status})`, `assignUserRole(id,roleId)`,
  `unassignUserRole(id,roleId)` (`@/api/access`); `useAddMember` (upsert/đổi role) +
  `useRemoveMember` (`@/api/members`, AX-1). Bulk = loop.
- Badge "active" của Users dùng token primary/teal (trùng màu CTA) → state đọc như action.
  Token `--success` (thêm ở AX-1) chưa dùng ở đây.

## Thiết kế

### 1. `FilterableTable` — optional selection
Thêm 1 prop:
```ts
selection?: { selected: Set<string>; onToggle(id): void; allSelected: boolean; onToggleAll(): void };
```
Khi có → prepend cột checkbox: header = select-all `Checkbox`, mỗi hàng = checkbox
(`checked={selected.has(getId(row))}`, toggle hàng). Không có → y như cũ. Selection do
`useBulkSelect` của page điều khiển; select-all chỉ tác động **hàng đang hiện** (đã
search/facet) — đúng như `toggleAll` với `rows` hiện. Giữ FilterableTable đơn nhiệm
(display + search + sort + optional selection).

### 2. Bulk trên Users + Workspace Members
Cả 2 trang đã dùng `FilterableTable`. Mỗi trang:
- Nút **"Bulk select"** toggle `bulkMode` (như list Notebooks); vào bulk mode → truyền
  `selection` cho FilterableTable (hiện cột checkbox).
- **Bulk-action bar** hiện trên bảng khi `someSelected`: "N selected" + actions + "Clear".

| Trang | Bulk actions |
|---|---|
| **Users** (`.../access/UsersPage.tsx`) | **Disable**/**Enable** users chọn (`updateUser(id,{status})`); **Assign role…** — chọn role, `assignUserRole(id,roleId)` mỗi id (admin-only) |
| **Workspace Members** (`.../explore/settings/Members.tsx`) | **Remove** members chọn (`useRemoveMember`); **Set role…** — chọn ws-role, add mỗi id (`useAddMember`, upsert) |

Mỗi bulk op chạy mutation single/mỗi id (tuần tự hoặc concurrency giới hạn), rồi invalidate
list query + clear selection. Tôn trọng guard: bulk **Remove** members không được lấy đi
admin cuối — backend trả `Conflict` per-item → item đó vào summary partial-failure (dưới),
không âm thầm thành công.

### 3. Undo (sonner toast + inverse)
Trước bulk op **reversible** → snapshot state cũ của các hàng tác động. Xong → toast vd
*"Disabled 4 users. [Undo]"*; bấm Undo → chạy inverse mỗi id: enable↔disable
(`updateUser`); assign→`unassignUserRole`; remove-member→re-add với role cũ đã snapshot
(`useAddMember`); set-role→set lại role cũ. Toast tự tắt ~6s. Chỉ op reversible mới có Undo.

**Partial failure:** bulk op chạy từng item độc lập, gom success + fail. Nếu có fail →
toast *"Disabled 3 of 4 users; 1 failed."* (variant error), và Undo (nếu có) **chỉ invert
các item thành công** (snapshot theo success). Không bao giờ nuốt lỗi.

### 4. Success-token state badge
Đổi badge "active" của Users sang token `--success` (variant `bg-success
text-success-foreground` hoặc inline `var(--success)`) → hàng active đọc như *state*, không
phải CTA. "disabled" giữ muted/secondary. Thay đổi màu nhỏ, độc lập; không đụng badge khác.

### 5. Skeleton + empty-state nhất quán
Thay text `"Loading…"` trên Users/Groups/Roles/Members bằng `ListSkeleton` (theo số cột)
khi list query loading. Giữ empty ("No users." …) + error; chỉ đổi loading → các list load
nhất quán.

## Kiểm thử (Vitest)
- **FilterableTable selection:** có `selection` → cột checkbox hiện; toggle hàng gọi
  `onToggle(id)`; select-all gọi `onToggleAll`; không có prop → không có cột checkbox (regression).
- **Users bulk:** vào bulk mode + chọn 2 user + "Disable" gọi `updateUser` cho cả 2 với
  `status:"disabled"`; action Undo gọi `updateUser` `status:"active"` cho các id đó;
  "Assign role…" gọi `assignUserRole` mỗi id.
- **Members bulk:** chọn members + "Remove" gọi remove/mỗi id; Undo re-add với role cũ.
- **Partial failure:** 1 item reject → summary "N of M" và Undo chỉ invert id thành công.
- **Badge:** active dùng success token (assert class/style).
- **Skeleton:** khi loading, `ListSkeleton` (`data-testid="list-skeleton"`) hiện thay "Loading…".

## Ngoài phạm vi
- Bulk **delete** Roles/Groups — destructive + guard seeded/last-admin; roadmap chỉ khoanh
  users/members. Không làm.
- Bulk endpoint backend — loop single đủ cho admin tool ở N thực tế.
- Bulk trên Permissions (catalog read-only — không có gì để tác động).

## Ghi chú
- Tái dùng: `useBulkSelect`, `ListSkeleton`, pattern bulk-toggle + bulk-bar của list
  Notebooks (`frontend/src/routes/explore/Notebooks.tsx`) làm tham chiếu tương tác,
  `FilterableTable` (AX-2), shadcn `Checkbox`/`Button`, `toast` sonner, token `--success` (AX-1).
- Giữ logic bulk mỗi page gọn: helper `runBulk(ids, op)` map ids → mutation single, gom kết
  quả, toast summary + Undo tùy chọn. Nếu file page phình, tách bulk-bar thành component
  in-file — không tái cấu trúc dialog/data hooks.
