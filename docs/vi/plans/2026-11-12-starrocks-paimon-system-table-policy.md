# StarRocks fork patch 0010 — Paimon system tables dưới policy base table — Kế hoạch

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-12-starrocks-paimon-system-table-policy.md) — mọi code, lệnh và giá trị chính xác nằm ở bản tiếng Anh; khi khác nhau thì bản tiếng Anh đúng. Thực thi theo SDD (subagent-driven-development).

**Mục tiêu:** thêm patch `0010` để user bị áp row/column policy trên base table Paimon `t` không đọc được system tables của `t` (`t$files/$snapshots/$partitions/…`).

**Cách làm:** overlay trong `ColumnPrivilege` (vòng lặp đầu, cạnh nhánh `MetadataTable` của `0001`): nếu `PaimonTable` là system table (`Identifier.isSystemTable`) → refuse khi `isRestrictedByPolicy(base)` (tái dùng helper + error code `ERR_ACCESS_DENIED_METADATA_TABLE_UNDER_POLICY` của `0001`). Không thêm base-SELECT, không đụng vòng lặp hai.

## Ràng buộc chung

- Làm trong repo fork `egret-starrocks`, cây `work/starrocks` (upstream `4.1.7`). `0010` áp sau `0001`.
- Overlay: KHÔNG base-SELECT, KHÔNG đổi authz theo tên, KHÔNG thêm error code. Phủ mọi system table qua `Identifier.isSystemTable()`.
- `EGRET_PATCH_LEVEL` 4→5, tag `4.1.7-egret.5`. Bump pin ở platform là follow-up riêng (ngoài phạm vi).
- Gate: probe `C14-paimon` = ENFORCED trên egret.5, vẫn LEAK trên official; `run.sh` 15/15.

## Task 1: Overlay ColumnPrivilege + unit test

Sửa `ColumnPrivilege.java`: thêm import `PaimonTable` + `org.apache.paimon.catalog.Identifier`; thêm nhánh `instanceof PaimonTable && isPaimonSystemTable` → `refusePaimonSystemTableUnderPolicy`, `continue`; thêm 2 helper (`isPaimonSystemTable`, `refusePaimonSystemTableUnderPolicy` — bỏ qua `isMetadataContext`, suy base qua `Identifier.getTableName()`, refuse nếu `isRestrictedByPolicy`). Thêm test `ColumnPrivilegePaimonSystemTableTest` (subclass `PaimonTable` override 2 getter tên; 5 ca: row policy → refuse, mask → refuse, không policy → qua, mask rỗng → qua, metadata context → qua). Chạy `mvn -pl fe-core test -Dtest=ColumnPrivilegePaimonSystemTableTest` (5 xanh). Mutation check: đặt `isRestrictedByPolicy` = false → 2 ca refuse phải FAIL, khôi phục.

## Task 2: Sinh patch 0010 + bump level + re-apply sạch

Commit 2 file trong `work/starrocks`, `git format-patch -1 --zero-commit` → `patches/0010-Check-a-Paimon-base-table-policy-for-its-system-table.patch`. Sửa `UPSTREAM` `EGRET_PATCH_LEVEL` 4→5. `rm -rf work/starrocks && scripts/apply.sh` (0001..0010 áp sạch, exit 0). Chạy lại unit test trên cây mới. Commit patch + UPSTREAM.

## Task 3: Build egret.5 + đo gate + README

`scripts/build-fe.sh` + `scripts/build-images.sh` (tag `4.1.7-egret.5`). Chạy probe nhóm catalog trên egret.5 → `C14-paimon` ENFORCED, 0 BLIND/0 ERROR, sight alice xanh; trên official → vẫn LEAK. `run.sh` 15/15. Thêm dòng `0010` vào bảng Patches + dòng kết quả e2e `egret.5` trong README. Commit.

## Ngoài phạm vi (follow-up riêng)

Bump pin platform + push/tag image GHCR; đóng FU-1 + known-issue C14-paimon khi egret.5 là mặc định; biến thể value-statistics qua Java/Flink writer (đã được refuse phủ, chưa đo).
