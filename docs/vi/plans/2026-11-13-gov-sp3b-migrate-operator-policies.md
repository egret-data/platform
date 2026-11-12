# Governance SP3b — Trạng thái migrate-complete per-EP — Kế hoạch

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-13-gov-sp3b-migrate-operator-policies.md) — mọi code, lệnh, signature chính xác ở bản tiếng Anh; khi khác nhau thì bản tiếng Anh đúng. Thực thi theo SDD.

**Mục tiêu:** phơi bày, với mỗi enforcement point, còn operator (manual) policy nào CHƯA migrate vào model hay không — tín hiệu gate per-EP của SP3d sẽ dùng, và badge operator đọc trên tab Migrate.

**Bối cảnh:** Migrate flow (classify operator policy → grant/rule, provenance `perm.migrated_from`, undo có hash-guard, equivalence-gate probes) ĐÃ CÓ sẵn từ SP4d-3. SP3b chỉ thêm **một rollup thuần** trên output của classifier sẵn có — `ep_migration_status(&[ClassifiedPolicy])` — qua một endpoint read-only + badge trên tab Migrate. KHÔNG migrate gì, KHÔNG đổi artifact biên dịch, KHÔNG thêm logic classify.

## Ràng buộc chung
- **Dựa trên SP3a** (chưa merge, PR #854) → branch off `feat/gov-sp3a-ep-framing`, không phải `main`.
- **Additive + read-only.** Chỉ báo status; không migrate. Ranger/OPA artifact, `explain`, mọi golden **không đổi** — không bless lại fixture nào. Golden đổi = sai.
- **Decision ②:** demo stack đúng 1 operator policy (access phẳng `legacy: analysts read finance_pg`, catalog `finance_pg`, select, user `member-sso`) → classifier hiện tại đã chuyển thành grant. KHÔNG có shape engine-capability → SP3b KHÔNG build mapping operator-policy → typed-engine-privilege (hoãn tới khi có shape thật).
- Chỉ tính policy `operator`; bỏ qua `compiler`/`infra`.
- `complete` fail-closed: EP complete ⇔ MỌI operator policy đã migrate (`pending == 0` và không `blocked`).
- Tái dùng `classify::classify_service`; không nhân đôi logic classify.
- `ep_migration_status` là hàm thuần gate compile per-EP của SP3d sẽ gọi. SP3b phơi bày; KHÔNG wire gate (đó là SP3d).
- CI gate: `cargo fmt --check`, clippy, `committed_openapi_json_is_current`, generated-client digest, DB-gated Rust tests.
- `CARGO_TARGET_DIR=<worktree>/target-sp3b`. Commit co-author Claude Opus 5.5. Chỉ stage file đã đổi; không push.
- Thay đổi user-visible (badge) → cập nhật docs + screenshot (`egret-data/docs`, Platform).

## Tasks
1. **`ep_migration_status` (backend, thuần)** — `classify.rs`: thêm `EpMigrationStatus { complete, migrated, pending, blocked }` + `BlockedPolicy { id, name, reason }` + hàm partition operator policy (migrated/pending/blocked). Unit test: rỗng→complete; 1 pending→incomplete; 1 migrated→complete; 1 blocked→liệt kê + incomplete; compiler/infra bị bỏ qua.
2. **Endpoint `migration-status`** — `routes.rs`: handler `service_migration_status` + `#[utoipa::path]` (mirror `classify_service_policies`), route `GET .../services/{service_id}/migration-status`, đăng ký trong `ApiDoc` (paths + schemas). Regen `openapi.json` (gate `committed_openapi_json_is_current`) + `npm run gen:api` (digest test).
3. **Badge trên MigrateTab** — `MigrateTab.tsx`: đọc endmpoint, hiện badge "Migration complete" / "N to migrate" / "M blocked" + list. Test `MigrateTab.test.tsx` (3 case). Live check demo stack (`docs-legacy-trino` → "1 to migrate") + screenshot.
4. **Docs** — `egret-data/docs` governance: giải thích badge + screenshot; note forward tới gate SP3d.

## Ngoài phạm vi
Mapping operator→typed-engine-privilege (decision ②); wire gate + switch demo (SP3d); clearance/role/workspace (SP3c); mọi thay đổi migration execution/undo/probes/artifact.
