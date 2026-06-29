# Catalog — lọc list table/view theo grant của token — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-29-catalog-list-filter-by-grant-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-29 · **Branch:** `fix/catalog-list-filter-by-grant` (off main). Gỡ "known limitation" của token-grants (PR #90): token scoped tới table cụ thể vẫn **thấy tên** table khác khi list.

## Vấn đề
Token grant `{sale,"orders",[read]}` bị từ chối truy cập table anh em (đúng), nhưng `GET …/namespaces/sale/tables` vẫn trả tất cả tên table (`orders2`, `leads`). Lộ thông tin (enumerate tên).

## Gốc rễ (đã xác minh trong lakekeeper fork)
`list_entities!` của lakekeeper CÓ lọc từng entity, nhưng bị short-circuit: (1) check `can_list_everything = is_allowed_namespace_action(ListEverything)`; (2) **nếu ListEverything được cấp → trả TẤT CẢ, bỏ qua check từng table**; (3) chỉ khi ListEverything bị từ chối → mới lọc từng entity qua `IncludeInList` (→ `egret` `gate_table`/`decide_table`, đã đúng). egret hiện cấp `ListEverything` qua `decide_ns_read` (token nào đọc được ns đều qua) → short-circuit luôn bật → lộ tên.

## Quyết định (phẫu thuật, chỉ `authorizer.rs`)
Xử lý riêng `CatalogNamespaceAction::ListEverything`: chỉ cho phép khi token có **grant whole-namespace read** phủ ns (`table==None`, có read, covers_ns) — dùng `gate_object(metadata, ns, false)` thay vì `gate_namespace`/`decide_ns_read`.
```rust
match action {
    CatalogNamespaceAction::ListEverything => self.gate_object(metadata, levels, false),
    _ => self.gate_namespace(metadata, levels, namespace_action_is_write(action)),
}
```
Các action khác giữ nguyên. (`gate_object`/`decide_object_ns` đã có sẵn.)

### Kết quả
- Grant whole-ns `{sale,null,[read]}` → ListEverything ✓ → list tất cả table sale (đúng).
- Grant table-specific `{sale,"orders",[read]}` → ListEverything **✗** → lakekeeper lọc từng table qua IncludeInList → chỉ `orders`; ẩn `orders2`/`leads`. **Hết leak.**
- `*` grant → ListEverything ✓ (covers qua `*`).
- Views: cùng macro → grant table-only không list view; whole-ns grant thì có.
- Gate endpoint (`ListTables`/`ListViews`/`ListNamespaces`) vẫn `decide_ns_read` → token table-only vẫn gọi được list (chỉ nội dung bị lọc).
- Admin Browse UI: dùng system principal → bypass authorizer → không ảnh hưởng.

## Components
`crates/egret-catalog-engine/src/authorizer.rs` — nhánh `ListEverything` trong `are_allowed_namespace_actions_impl` (thay đổi hành vi duy nhất) + unit test.

## Test
Unit: token table-only → `decide_object_ns(sale,read)` **false** nhưng `decide_ns_read(sale)` + `decide_table(sale,"orders",read)` **true**; token whole-ns → `decide_object_ns(sale,read)` **true**. Live REST e2e (bằng chứng chính): token `[{sale,null,[read]},{mkt,"campaigns",[read,write]}]` → `list mkt/tables` chỉ `campaigns` (không `leads`), `list sale/tables` đủ; deny không đổi (load `mkt.leads` 404). Full CI (authz): fmt + clippy --workspace + `cargo test -p egret-catalog-engine` (gồm live front_auth). Manual Trino offer.

## Rủi ro
Under-list (whole-ns phải vẫn list đủ — có test); endpoint không được 403 (gate là ListTables, không phải ListEverything — live e2e xác nhận 200); phụ thuộc hành vi short-circuit của fork → live e2e là chốt chặn.

## Ngoài phạm vi
Lọc list child-namespace theo grant (tên ns không truy cập được vẫn có thể hiện); column-level; wildcard; TTL.
