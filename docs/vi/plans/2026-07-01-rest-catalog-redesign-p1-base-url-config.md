# Redesign REST Catalog — Plan 1: base_url + tách config (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-01-rest-catalog-redesign-p1-base-url-config.md` (code/step đầy đủ ở đó).
> Spec: `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md` (Workstream **A** + **B**). Thực thi qua subagent-driven-development.

**Mục tiêu:** lakekeeper nhúng advertise **đúng public catalog URL tại nguồn** (bỏ rewrite band-aid #114), và **tách `catalog.public_url` khỏi kernel `api_url`**.

## Task
- **T1 (fork lakekeeper):** fork `lakekeeper/lakekeeper` → `egretdb/lakekeeper` tại rev pin `791650571…`; thêm `pub fn with_base_url(mut self, base_url) -> Self` vào `crates/lakekeeper/src/request_metadata.rs` (unit-test); push; thêm `[patch."https://github.com/lakekeeper/lakekeeper"]` trỏ fork trong Cargo.toml root; `cargo build -p egret-catalog-engine`.
- **T2 (stamp base_url):** `token_request_metadata(auth, base_url)` chain `.with_base_url(...)`; caller trong `front_auth()` lấy `state.catalog_public_url`; **cũng stamp ở site system + browse** (`browse.rs`/`bootstrap.rs`) — *cần định vị chính xác chỗ dựng `RequestMetadata` lúc implement*. Test `token_metadata_carries_configured_base_url`.
- **T3 (xoá rewrite #114):** xoá `LAKEKEEPER_INTERNAL_CATALOG_BASE`, `rewrite_lakekeeper_base`, 3 test rewrite, block response-rewrite cuối `front_auth()` → trả `Ok(next.run(req).await)`. Giữ `FrontAuthState.catalog_public_url` (giờ dùng cho T2).
- **T4 (base_uri env):** trong `main.rs` set `LAKEKEEPER__BASE_URI=catalog_base_url` trước khi build engine; verify manual `/config` `overrides.uri` không còn `:8181`.
- **T5 (tách config, Workstream B):** thêm `ServerConfig.public_url: Option<String>` (default None); `api_url` derive từ `server.public_url` (fallback `http://127.0.0.1:{port}`); `catalog_base_url` vẫn từ `catalog.public_url`. Test default None.
- **T6 (verify):** fmt + clippy + unit (`egret-catalog-engine`, `egret-config`) + integration DB-gated (`front_auth`, `gc_e2e`).

## Ghi chú
- T1 là root-fix (không dùng fallback rewrite). Fork cần host tại `egretdb/lakekeeper`; bump lakekeeper phải re-apply patch.
- T2 site system/browse cần đọc code chính xác lúc làm.
- Đây là 1/6 plan (A→B→**D→E→C→F** viết tiếp sau).
