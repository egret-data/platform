# Thiết kế lại REST Catalog — Thiết kế (bản tham chiếu)

> Bản chuẩn: `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md`. File này rút gọn.
> Ngày 2026-07-01 · **Sửa 2026-07-02 (pivot single-catalog + ghi-qua-engine)** · Trạng thái: đã duyệt, write path **spike-validated (1a/1b)**.

## Mục tiêu
Rút gọn về **MỘT catalog Iceberg duy nhất, engine-facing**, và cho notebook **ghi qua query engine** (Daft `write_sql`) — notebook **không chạm** REST catalog trực tiếp. Bỏ hẳn thiết kế 2-catalog / per-user-JWT / vended-STS: catalog chỉ cần auth **opaque, table-level**; phân quyền dữ liệu enforce **một lần** ở engine (Ranger/ACL) cho cả đọc + ghi. Đồng thời fix nốt bug plumbing (base_url, config overload) + dọn catalog cũ.

## Hiện trạng
- lakekeeper nhúng tại `/catalog/v1`, `front_auth` hybrid (JWT `aud=egret-catalog` / opaque token). `egret-catalog` = identity. REST hand-rolled cũ (`egret-catalog::api`, `egret-iceberg-ext`) = di sản.
- Trước pivot: notebook đọc qua `trino()`/`starrocks()` nhưng **ghi** bằng Daft-direct Iceberg → cần giữ credential catalog + storage → nguồn của toàn bộ độ phức tạp.

### Bug plumbing còn fix
1. **base_url `:8181`** rò rỉ trong `/config` `overrides.uri` — **Trino Iceberg REST connector cũng follow** → vẫn phải fix.
2. **`catalog.public_url` overload** cả kernel `api_url`.
3. **Di sản kép** hand-rolled.

## Pivot — 1 catalog, ghi qua engine
**Chốt (2026-07-02):** notebook đọc qua `trino()`/`starrocks()`, **ghi** qua helper **`write_table`** (bọc `df.write_sql` trên cùng conn engine per-user JWT). **Một catalog chung**; notebook chỉ là client engine.
**Vì sao:** xoá phần phức tạp nhất (catalog JWT, STS, catalog notebook riêng, credential-separation); và vì ghi cũng qua engine → **governance đồng nhất**: một policy Ranger/ACL cho cả read+write, không còn đường Daft-direct để lệch.
**Đã validate (spike live):** 1a `write_sql`→Trino `memory`; 1b `write_sql`→Trino→**Iceberg REST→lakekeeper→MinIO**→read back.
**Đánh đổi:** `write_sql` = `pandas.to_sql` per-partition → hợp EDA-scale (summary, feature table), không phải bulk; `write_sql` "early dev".

## Governance
- **Một điểm enforce = engine** (Trino/StarRocks + Ranger/ACL) cho mọi read/write. Notebook không lách được vì không có đường catalog/storage trực tiếp.
- **REST catalog không phải governance surface cho user** — chỉ auth **service credential** (opaque, table-level) + admin.
- **Hướng C (catalog FGAC, Polaris #137)** chỉ là future note — không còn gap cần đóng.

## Credential model
| Mặt phẳng | Credential | Verify | Scope |
|---|---|---|---|
| Notebook read | JWT `aud=trino`/`starrocks` | engine + Ranger/ACL | governed reads |
| Notebook write (`write_table`) | **cùng** JWT engine | engine + Ranger/ACL | governed writes |
| Engine→catalog (service) | **opaque token table-level** | egret `front_auth` | warehouse chung |
| Admin/warehouse-mgmt | opaque token | `front_auth` | per warehouse |
Notebook **không dùng** JWT `aud=egret-catalog` nữa.

## Kiến trúc (đã sửa)
- **A. base_url tại nguồn** (fork lakekeeper + `LAKEKEEPER__BASE_URI`, xoá rewrite #114) — **GIỮ** (Trino follow `overrides.uri`). Xem Plan 1.
- **B. Tách config** (`catalog.public_url` ≠ `server.public_url`/kernel api_url) — **GIỮ**. Plan 1.
- **W. Write path** (`kernel-image`) — **XONG**: `write_table(df,"catalog.schema.table",engine=,mode=)` bọc `write_sql`; parse tên → default catalog/schema + `CREATE SCHEMA IF NOT EXISTS`; thêm `pandas`+`trino` vào image. Đã commit + spike-validate. StarRocks chưa validate.
- **R. Xoá Daft-direct**: bỏ `iceberg()`/`write_iceberg()`/`catalog()`/`_resolve_identifier` (+ export/startup/test); `build_notebook_runtime`/`provision_notebook_access` **bỏ provision token+namespace catalog**; rewrite e2e sang `trino()`+`write_table`.
- **C. Một catalog chung**: bỏ khái niệm notebook-purpose warehouse; `front_auth` giữ path opaque (service+admin); path JWT-catalog per-user notebook không dùng.
- **F. Dọn di sản** (`egret-catalog::api`, `egret-iceberg-ext` thừa; verify `codegraph_impact`).
- **Prerequisite (ops)**: đăng ký catalog Iceberg trong Trino/StarRocks (REST + opaque service token + S3/MinIO); **Trino JWT→group provider** (dev ACL từ chối ghi vì không biết group của user — bắt buộc cho governed write).

## Data flow
1. Đọc: `trino("...FROM lakehouse.sales")` → JWT `aud=trino` → Ranger/ACL → Daft DF.
2. Ghi: `write_table(df,"lakehouse.analytics.summary")` → CREATE SCHEMA → `write_sql` qua Trino → CREATE TABLE+INSERT vào Iceberg (lakekeeper) → file S3 — governed như read.
3. Chia sẻ: user khác `trino("...FROM lakehouse.analytics.summary")`.

## Test
- Unit kernel (xong): `write_table` qualify tên/bare-name/CREATE SCHEMA/dispatch/reject non-3-part.
- Unit backend: config split; `with_base_url`.
- e2e opt-in: rewrite `notebook-dataframe.spec.ts` sang `write_table`+`trino()`; giữ guard reopen+rerun (giờ xanh hiển nhiên — không còn token catalog per-kernel để revoke).
- Xoá: test Daft-direct + test rewrite #114 (theo Plan 1).

## Deps
Kernel image: thêm `pandas` (`write_sql`→`to_sql`) + `trino` (dialect + `JWTAuthentication`; trước bị thiếu).

## Ngoài phạm vi
Hướng C; ghi bulk/lớn từ notebook; cấu hình đăng ký catalog + Ranger/group-provider (ops).

## Rủi ro
- `write_sql` early-dev; pin theo Daft cài, re-verify khi bump.
- Trần EDA-scale (`to_sql` per-partition).
- **base_url vẫn bắt buộc** (Trino follow `overrides.uri`) — Plan 1 (A) không optional.
- **Trino group-mapping** bắt buộc cho governed write (dev thiếu; spike nới ACL tạm).
- StarRocks write chưa validate — ưu tiên `engine="trino"`.
- **Thứ tự (sửa):** W (xong) → A+B → R (xoá Daft-direct + rewrite e2e) → C (dọn single-catalog) → F. **Bỏ P2 (STS) + P4 (notebook-JWT); P3 (purpose) bị supersede bởi C.**
