# StarRocks fork patch 0010: Paimon system tables dưới policy của base table (thiết kế)

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/specs/2026-11-12-starrocks-paimon-system-table-policy-design.md). Mọi giá trị chính xác, tên lớp/hàm và chi tiết test nằm ở bản tiếng Anh; khi hai bản khác nhau thì bản tiếng Anh đúng.

**Trạng thái:** thiết kế, 2026-11-12. Follow-up FU-1 của gói thăm dò các đường chưa đo (gói C).

## Vấn đề

System tables của một bảng Paimon — `t$files`, `t$snapshots`, `t$partitions` và các bảng `t$…`
khác — để lộ dữ liệu của base table `t`: giá trị partition (kể cả partition mà row filter định
giấu), đường dẫn file object-store, record count, và lower/upper bound từng cột (giá trị thật của
cột, với bảng có value statistics).

Patch `0001` đã chặn việc này cho **metadata table** gốc của StarRocks: đọc một metadata table
cần `SELECT` trên base table VÀ bị từ chối nếu user bị áp row access policy hoặc column masking
policy trên base table. Nhưng `0001` chỉ gắn vào nhánh `table instanceof MetadataTable` trong
`ColumnPrivilege`. Một Paimon system table **không** resolve thành `MetadataTable`: connector
Paimon trả về nó như một `PaimonTable` thường, tên `orders$files` — nên nó bỏ qua check của
`0001` và chỉ bị kiểm quyền theo chính tên đó.

Đã đo (check **C14-paimon**, 2026-11-09) trên **cả hai** image official `4.1.7` và fork
`4.1.7-egret.4`: `bob` (có row filter `region='us'` + column mask trên `paimon.sales.orders`) đọc
được mọi dòng của `orders$files/$snapshots/$partitions` — partition `eu`, đường dẫn `s3://…`,
record count. `eve` (không grant) bị DENIED đúng. Vậy leak là **bypass policy**, không phải thiếu
grant. Đây là giả thuyết **H7**, đã xác nhận; là hành vi có sẵn của StarRocks, không phải fork gây ra.

## Mục tiêu

User bị áp row access policy hoặc column masking policy trên base table Paimon `t` **không** đọc
được system tables của `t`. Fix sao chép đúng hành vi đã-duyệt của `0001` sang code path Paimon.
Không đổi với user có `SELECT` không hạn chế; không đổi trên image official (không có patch).

## Ý chính thiết kế

Một patch `patches/0010-…patch` sửa `ColumnPrivilege.java` + thêm một unit test, phụ thuộc `0001`
(tái dùng `isRestrictedByPolicy` + error code) và áp sau nó.

**Đây là một overlay, KHÔNG phải mirror đầy đủ của `0001`.** Khác biệt then chốt: Paimon system
table **đã** được authz theo tên riêng bởi `Authorizer` chuẩn (probe đo được: grant `table=orders`
→ DENIED trên `orders$files`; `table=*` → đọc được). Nên `0010` **không** thêm yêu cầu `SELECT` trên
base như `0001`, và **không** đổi grant nào với tới system table. Nó chỉ thêm đúng một điều: system
table của base `t` bị từ chối với user bị áp **row policy hoặc column mask trên `t`** — đúng leak đã đo.

- **Hook:** trong vòng lặp đầu của `ColumnPrivilege`, thêm nhánh `table instanceof PaimonTable` và là
  system table → gọi `refusePaimonSystemTableUnderPolicy`, `continue` (bỏ rewrite mask/filter cho
  system table). Authz theo tên đã chạy trước đó nên không cần đụng tới.
- **Phát hiện system table + base table:** dùng `Identifier` có sẵn: `isSystemTable()` đúng cho mọi
  `t$…`; `getTableName()` trả base `t`. Phủ đồng loạt mọi system table.
- **Check:** bỏ qua khi `isMetadataContext()`; nếu `isRestrictedByPolicy(base)` (row policy hoặc
  column mask) → ném `ERR_ACCESS_DENIED_METADATA_TABLE_UNDER_POLICY`. **Không** gọi `checkTableAction`
  SELECT trên base (đó là hành vi của `0001` cho metadata table tổng hợp; thừa và gây mở rộng quyền ở
  đây vì Paimon system table đã authz theo tên).
- **Vì sao patch mới `0010`:** code path khác (`PaimonTable` vs `MetadataTable`), finding riêng; mỗi
  patch một concern. `0010` tái dùng helper + error code của `0001` nên đứng sau `0001`.

## Không làm (non-goals)

- Áp filter/mask lên chính rows của system table (cột khác nhau, không map được) — ta refuse như `0001`.
- Value statistics cột trong `$files` qua Java/Flink writer: chưa đo (seed pypaimon ghi null stats),
  nhưng đã được phủ bởi hành vi refuse-under-policy. Đo thêm cần Spark/Flink (ngoài phạm vi).
- Paimon branch tables / time-travel: concern khác, ngoài phạm vi.
- Image official: không thuộc build fork; giữ nguyên làm baseline (vẫn leak).

## Test

- **Unit (fe-core):** kiểu `ColumnPrivilegeMetadataTableTest` cho một `PaimonTable` tên
  `orders$files`: refuse khi có row policy; refuse khi có column mask; cho qua khi không policy
  (overlay không thêm yêu cầu base-SELECT); `isMetadataContext` không bị refuse; bảng Paimon thường
  (`orders`) không bị nhánh mới chạm.
- **E2e (probe):** gate là check **C14-paimon**. Sau `0010`: trên fork `4.1.7-egret.5` → bob/eve
  DENIED → **ENFORCED**; trên official (không patch) → vẫn **LEAK** (bằng chứng check phát hiện đúng
  thứ patch sửa); sight control `alice` vẫn xanh. Bộ `run.sh` 15/15 phải vẫn xanh.

## Ship

- Thêm `patches/0010-…patch`; `UPSTREAM` `EGRET_PATCH_LEVEL` 4 → 5; build + tag `4.1.7-egret.5`
  (FE và allin1); một lần bump pin ở platform. README fork: thêm `0010` vào bảng Patches + dòng kết
  quả e2e cho `egret.5`. Đóng FU-1 + known-issue C14-paimon khi `egret.5` là image mặc định.
