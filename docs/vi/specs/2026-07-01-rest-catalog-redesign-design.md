# Thiết kế lại REST Catalog — Thiết kế (bản tham chiếu)

> Bản chuẩn: `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md`. File này rút gọn.
> Trạng thái: đã duyệt (brainstorming) · 2026-07-01.

## Mục tiêu
Thiết kế lại tích hợp Iceberg REST catalog của egret để **fix tận gốc 5 vấn đề plumbing** (PR #114 chỉ vá tạm) VÀ thiết lập **model governance** giữ phân quyền nhất quán trên mọi đường đọc. Hợp nhất auth notebook về **JWT delegation**, dùng **vended STS creds**, tách config overload, sửa **base_url tại nguồn**, chọn warehouse theo **purpose**, dọn catalog hand-rolled, và enforce "**governed data chỉ đọc qua engine**" bằng **tách credential**.

## Hiện trạng
- Catalog thật = **lakekeeper nhúng** (pin `0.12.3` rev `79165057`) tại `/catalog/v1`, đứng trước là `front_auth`. `egret-catalog` = identity (opaque `catalog_tokens` + map warehouse). REST hand-rolled cũ (`egret-catalog::api`, `egret-iceberg-ext`) = di sản.
- `front_auth` **hybrid**: bearer là JWT (3 đoạn) → `CatalogJwtAuth` (`aud=egret-catalog`, như Trino/StarRocks); còn lại → opaque `catalog_tokens`. Notebook đang dùng **opaque** (mint-and-revoke).

### 5 vấn đề plumbing
1. **base_url `:8181`** — hardcode trong `RequestMetadata`, không setter; `/config` `overrides.uri` + `s3.signer.uri` trỏ `:8181` → client không tới được. #114 rewrite = band-aid.
2. **`catalog.public_url` overload** cả kernel `api_url` → set đúng catalog thì hỏng callback engine.
3. **mint-and-revoke → 401 flaky** — token kernel đang chạy bị revoke khi manifest fetch lại.
4. **Remote signing thừa** — `/signer/...` (round-trip + nguồn bug ownership #114).
5. **Di sản kép** — REST hand-rolled cũ song song lakekeeper.
- Kèm: chọn catalog notebook theo **tên + fallback-first** = dễ sai.

## Governance model (kim chỉ nam)
Mục tiêu: **phân quyền một lần — đọc engine nào cũng cùng kết quả.** Ràng buộc: **column/row-level chỉ enforce được qua query engine** (Trino/StarRocks + Ranger viết lại query); reader đọc-file-trực-tiếp (Daft) + vended STS chỉ tới **table/prefix-level**; catalog REST (lakekeeper) chỉ **table-level**.

### Hướng A — governed data engine-only (CHỐT)
Bỏ đường đọc thứ hai cho governed data; **một điểm enforce = Ranger qua engine.** Hai catalog, **cả hai đăng ký trong Trino/StarRocks**:

| | `lakehouse` (governed) | `notebook` (EDA outputs) |
|---|---|---|
| **Daft-direct** | ❌ credential không có grant | ✅ rw namespace của mình |
| **Qua Trino/StarRocks** | ✅ đọc, Ranger row/column | ✅ đọc (chia sẻ), Ranger áp |
| **Pipeline/BI qua engine** | ✅ đọc-ghi | ✅ đọc output notebook |

**Enforce bằng TÁCH CREDENTIAL, không phải quy ước:** JWT catalog của kernel (`aud=egret-catalog`) chỉ có quyền trên warehouse **`notebook`**, không có grant trên warehouse governed → `iceberg("lakehouse.t")` bị catalog **từ chối (403)**. Đường duy nhất tới `lakehouse` = `trino()`/`starrocks()` (JWT `aud=trino/starrocks` + Ranger). Một policy Ranger, mọi read governed đi qua đó → nhất quán by-construction.
**Đánh đổi:** notebook mất tốc độ Daft-direct trên lakehouse (đọc governed qua engine); dữ liệu của mình (`notebook`) vẫn full Daft-direct; đọc chéo output notebook governed chỉ khi qua engine.

### Hướng C — catalog-native FGAC (future, KHÔNG phụ thuộc)
Lật nguồn-sự-thật về **catalog**: catalog giữ policy row/column, trả **scan đã governed** qua REST scan-planning (`planTable`/`fetchScanTasks`) → mọi reader (kể cả Daft-direct) nhất quán từ 1 policy catalog. Đây là hướng ngành ([Polaris #137](https://github.com/apache/polaris/issues/137) — **open, chưa PR, chưa ship**; lakekeeper cũng chưa có) + cần Daft biết honor filter/mask (chưa có). **Không cược thiết kế vào nó, không đổi catalog.** Giữ abstraction catalog sạch + tách rõ đường governed-read để sau này adopt C không phải đập lại.

## Credential model (3 mặt phẳng, 1 identity)
Cả 3 là JWT per-user từ 1 identity Keycloak, mint bởi `EngineTokenMinter` (token-exchange), khác nhau `aud`, refresh cùng cơ chế:

| Mặt phẳng | Credential | Verify bởi | Scope |
|---|---|---|---|
| Catalog (Daft-direct) | JWT `aud=egret-catalog` | egret `front_auth` | rw namespace của mình trong warehouse `notebook` |
| Trino | JWT `aud=trino` | Trino (JWKS) + Ranger | đọc governed (lakehouse+notebook) |
| StarRocks | JWT `aud=starrocks` | StarRocks (JWKS) + Ranger | đọc governed |

**Opaque token** chỉ giữ cho admin/warehouse-mgmt/non-OIDC — chỉ nói chuyện với catalog REST của egret, **không** dùng được cho Trino/StarRocks (chúng chỉ nhận JWT Keycloak).

## Kiến trúc (6 workstream)
- **A. base_url tại nguồn** (`egret-catalog-engine` + fork): thêm `with_base_url` vào `RequestMetadata`; set `LAKEKEEPER__BASE_URI` + 3 chỗ dựng metadata trong `front_auth` dùng public URL; **xoá `rewrite_lakekeeper_base`** + buffering. *Dự phòng: 1 lớp rewrite config-driven.*
- **B. Tách config** (`egret-config`, `egret-bin`): `config.catalog.public_url` (chỉ catalog) ≠ `config.server.public_url` (kernel `api_url`, default `http://127.0.0.1:{port}`).
- **C. Notebook → JWT + authz Hướng A** (`egret-kernel`, `egret-notebook`, helper, `egret-catalog-engine`): kernel lấy JWT `aud=egret-catalog` từ minter (extend `/kernel/engine-token` hoặc `/kernel/catalog-token`), cache+refresh; `catalog()` dùng JWT; **authz** = rule "subject rw `notebook_{preferred_username}` **chỉ trong warehouse `notebook`**, không có gì trên warehouse `shared`" → đây là chỗ chặn `iceberg("lakehouse..")` = 403; `provision_notebook_access` **bỏ mint+revoke opaque**, manifest chỉ mang `{url,warehouse,namespace}`.
- **D. Vended STS** (`egret-bin`, `egret-catalog-engine`, helper): storage-profile warehouse `notebook` bật vending + `remote-signing-enabled=false`; `/config`/loadTable vend STS scoped; **xoá nhánh signer** (`warehouse_prefix` về first-segment); kernel bỏ `AWS_*` env.
- **E. Warehouse purpose** (migration, `egret-notebook`, admin API/UI): cột `purpose` (`shared`|`notebook`, default `shared`); ≤1 `notebook`/workspace; `build_notebook_runtime` chọn theo purpose (không có → lỗi rõ, không fallback thầm); UI "Add warehouse" chọn purpose; backfill `name=="notebook"`. **Cả 2 catalog đăng ký trong engine** (ops-level, document).
- **F. Dọn di sản** (`egret-catalog`, `egret-iceberg-ext`): giữ identity layer; xoá REST hand-rolled + type không dùng (verify `codegraph_impact`).

## Data flow (phiên EDA)
1. Đọc governed: `trino("... FROM lakehouse.sales")` → JWT `aud=trino` → Ranger mask/filter → Daft DF. (`iceberg("lakehouse..")` = 403.)
2. Ghi output: `write_iceberg(df,"t")` → JWT `aud=egret-catalog` → `/config` (base_uri đúng + STS) → commit namespace của mình trong `notebook` → file S3 trực tiếp bằng STS (không signer).
3. Chia sẻ: user khác `trino("... FROM notebook.<user>.t")` → engine + Ranger.
4. Refresh: JWT/STS hết hạn → refresh → retry (mở lại notebook + re-run luôn chạy).

## Xử lý lỗi
JWT/STS hết hạn → refresh+retry; Daft-direct vào governed → 403 rõ ("đọc qua trino()/starrocks()"); không có warehouse notebook → lỗi rõ (không ghi thầm); OIDC tắt → path JWT notebook báo rõ, opaque admin không ảnh hưởng.

## Test
- **Unit:** config tách; `with_base_url` (metadata/`s3_signer_uri` đúng URL); rule authz rw `notebook_{self}` chỉ warehouse notebook + **deny warehouse `shared`** (biên Hướng A); `build_notebook_runtime` theo purpose (không có → lỗi).
- **Integration (DB-gated):** `front_auth` JWT e2e; JWT notebook 403 trên warehouse shared, 200 namespace mình; `/config` base_uri đúng + shape STS; hết signer.
- **e2e (opt-in):** roundtrip qua JWT+STS; **regression #3** mở lại+re-run không 401; **guard Hướng A** `iceberg(governed.t)` lỗi rõ còn `trino(...governed.t)` chạy.
- **Xoá:** test `rewrite_lakekeeper_base` + signer-path của #114.

## Ngoài phạm vi
Hiện thực Hướng C; cấu hình Ranger policy + đăng ký connector Iceberg cho engine (ops, document); Daft-direct đọc governed (cấm by-design); multi-region; schema evolution/partition khi ghi; auto-provision warehouse notebook.

## Rủi ro
- Fork lakekeeper theo rev pin; bump phải re-verify. Dự phòng: rewrite config-driven.
- Vended STS cần MinIO/S3 bật STS + storage-profile vending; store không hỗ trợ → fallback signing warehouse đó.
- `preferred_username` phải ổn định/không đụng nhau.
- Đăng ký catalog `notebook` trong engine là điều kiện để chia sẻ chéo; không đăng ký → chỉ Daft-direct own-namespace. (ops requirement.)
- **Thứ tự:** A → B → D → E → C (lớn nhất, mang biên governance) → F. A–B–D–E độc lập/correctness; C phụ thuộc B+D; F cuối.
- Back-compat: backfill purpose; flow opaque-token admin giữ nguyên.
