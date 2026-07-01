# Thiết kế lại REST Catalog — Thiết kế (bản tham chiếu)

> Bản chuẩn: `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md`. File này rút gọn.
> Trạng thái: đã duyệt (brainstorming) · 2026-07-01.

## Mục tiêu
Thiết kế lại phần tích hợp Iceberg REST catalog của egret để **fix tận gốc 5 vấn đề** lộ ra khi làm notebook Iceberg chạy live (PR #114 chỉ là vá tạm): hợp nhất auth notebook về **JWT delegation**, dùng **vended STS creds**, **tách config** overload, sửa **base_url lakekeeper tại nguồn**, chọn warehouse **tường minh**, và **dọn catalog hand-rolled cũ**.

## Hiện trạng
- Catalog thật = **lakekeeper nhúng** (pin `0.12.3` rev `79165057`), mount tại `/catalog/v1`, đứng trước là `front_auth` (token-ownership). `egret-catalog` = lớp identity (opaque `catalog_tokens` + map `workspace_warehouses`→lakekeeper UUID). REST hand-rolled cũ (`egret-catalog::api`, `egret-iceberg-ext`) là di sản.
- Notebook 2-catalog trên 1 lakekeeper: BI (Trino/StarRocks) → warehouse "shared"; notebook → catalog B = warehouse chọn theo **tên** `"notebook"` (fallback: cái đầu). `provision_notebook_access` mint namespace `notebook_{user}` + **opaque token rw 1-grant** (mint-and-revoke).

### 5 vấn đề
1. **base_url `:8181` rò rỉ** — `RequestMetadata.base_url` hardcode `:8181`, không có setter; egret inject metadata dựng sẵn → `/config` `overrides.uri` + `s3.signer.uri` trỏ `:8181` (client không tới được). PR #114 rewrite response = band-aid.
2. **`catalog.public_url` overload** cả kernel `api_url` → set đúng catalog thì hỏng callback engine/Trino/StarRocks.
3. **mint-and-revoke → 401 flaky** — token của kernel đang chạy bị revoke khi manifest fetch lại → re-run sau khi mở lại = 401.
4. **Remote signing thừa** — đi qua `/signer/...` (thêm round-trip + nguồn bug ownership vừa vá).
5. **Di sản kép** — REST hand-rolled cũ tồn tại song song lakekeeper → gây nhầm "2 catalog".
- Kèm: chọn catalog B theo **tên + fallback-first** = implicit, dễ sai.

## Quyết định (brainstorming)
1. **Giữ lakekeeper nhúng, fix tận gốc** (egret all-in-one; tách service/tự viết đều bất lợi hơn).
2. **Vended STS creds** (scoped theo bảng) cho data-plane; bỏ signer; kernel không giữ creds gốc. MinIO hỗ trợ STS.
3. **Notebook auth = JWT per-user** (`aud=egret-catalog`) như Trino/StarRocks, mint bởi `EngineTokenMinter` (token-exchange), refresh cùng cơ chế. Opaque token **chỉ giữ** cho admin/warehouse-mgmt/non-OIDC.
4. **Warehouse có `purpose` tường minh** — giữ multi-warehouse (isolation storage/governance) nhưng chọn theo purpose, không theo tên.
5. **base_url fix tại nguồn** = thêm setter nhỏ vào **fork lakekeeper đã pin** (tiền lệ mysql_async) + set từ config; **xoá rewrite PR #114**. *Dự phòng nếu không muốn fork: 1 lớp rewrite sạch, config-driven.*

## Kiến trúc (6 workstream)
- **A. base_url tại nguồn** (`egret-catalog-engine` + fork): thêm `with_base_url` vào `RequestMetadata`; set `LAKEKEEPER__BASE_URI` + 3 chỗ dựng metadata trong `front_auth` dùng public URL; **xoá `rewrite_lakekeeper_base`** + block buffering.
- **B. Tách config** (`egret-config`, `egret-bin`): `config.catalog.public_url` (chỉ catalog) ≠ `config.server.public_url` (kernel `api_url`, default `http://127.0.0.1:{port}`).
- **C. Notebook → JWT** (`egret-kernel`, `egret-notebook`, helper): kernel lấy JWT `aud=egret-catalog` từ minter (extend `/kernel/engine-token` hoặc `/kernel/catalog-token`), cache+refresh như engine; `catalog()` dùng JWT (path `CatalogJwtAuth`); **authz rw** = rule dẫn xuất "subject có rw trên `notebook_{preferred_username}`" trong warehouse notebook; `provision_notebook_access` **bỏ mint+revoke opaque**, manifest chỉ mang `{url,warehouse,namespace}`.
- **D. Vended STS** (`egret-bin`, `egret-catalog-engine`, helper): storage-profile warehouse notebook bật credential vending + `remote-signing-enabled=false`; `/config`/loadTable vend STS scoped; **xoá nhánh signer** trong `front_auth` (`warehouse_prefix` về first-segment); kernel bỏ `AWS_*` env.
- **E. Warehouse purpose** (migration `egret-metastore`, `egret-notebook`, admin API/UI): thêm cột `purpose` (`shared`|`notebook`, default `shared`); ràng buộc ≤1 notebook/workspace; `build_notebook_runtime` chọn theo purpose (không có → lỗi rõ, không fallback thầm); UI "Add warehouse" chọn purpose; migration backfill `name=="notebook"`→notebook.
- **F. Dọn di sản** (`egret-catalog`, `egret-iceberg-ext`): giữ identity layer; xoá REST hand-rolled chết + type không dùng (verify `codegraph_impact` trước khi xoá).

## Data flow (write→read)
`catalog()` lấy JWT (cache/refresh) → `/config` (base_uri đúng + STS creds) → table ops JWT-authz (`notebook_{user}` rw) → data files S3 trực tiếp bằng STS → JWT hết hạn thì refresh+retry (không 401 vĩnh viễn; mở lại notebook + re-run luôn chạy).

## Xử lý lỗi
- JWT/STS hết hạn → 401/auth error → kernel refresh → retry; lỗi helper `egret:` rõ ràng khi không cứu được.
- Không có warehouse notebook → lỗi rõ, không ghi thầm vào shared.
- OIDC tắt → path JWT notebook không khả dụng, báo rõ; path opaque admin không ảnh hưởng.

## Test
- **Unit:** config tách 2 field độc lập; `with_base_url` (metadata + `s3_signer_uri` phản ánh URL đúng); rule authz chỉ rw `notebook_{self}` (không của user khác/shared); `build_notebook_runtime` chọn theo purpose (không có → lỗi, không fallback).
- **Integration (DB-gated):** `front_auth` JWT e2e với lakekeeper nhúng; `/config` đúng `base_uri` + shape vended-creds; hết signer path.
- **e2e (opt-in, live):** roundtrip write→read qua JWT+STS; **thêm regression #3**: mở lại notebook (ép fetch manifest mới) + re-run → assert không 401.
- **Xoá:** test `rewrite_lakekeeper_base` + test signer-path của #114.

## Ngoài phạm vi
Đổi cấu hình connector Trino/StarRocks; governed cross-warehouse read từ notebook (vẫn Trino-only); multi-region storage; schema evolution/partition khi ghi; auto-provision warehouse notebook (admin tự tạo, nay có purpose).

## Rủi ro
- Fork lakekeeper cho setter phải theo rev pin; bump lakekeeper phải re-verify. Dự phòng: 1 lớp rewrite config-driven.
- Vended STS cần MinIO/S3 bật STS + storage-profile cấu hình vending; store không hỗ trợ STS → fallback remote-signing cho warehouse đó.
- `preferred_username` phải ổn định + không đụng nhau (đã giả định bởi `notebook_{username}` hiện tại).
- **Thứ tự:** A (base_url+xoá rewrite) → B (tách config) → D (STS+xoá signer) → E (purpose) → C (notebook→JWT, lớn nhất) → F (dọn). A–B–D–E độc lập/correctness; C phụ thuộc B+D; F cuối.
- Back-compat: workspace cũ backfill purpose; flow opaque-token admin giữ nguyên.
