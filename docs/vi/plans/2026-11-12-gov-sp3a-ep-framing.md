# Governance SP3a — Khung EP + version per-EP + matrix key image — Kế hoạch

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-12-gov-sp3a-ep-framing.md) — mọi code, lệnh, signature chính xác ở bản tiếng Anh; khi khác nhau thì bản tiếng Anh đúng. Thực thi theo SDD.

**Mục tiêu:** thêm chiều **egret image-level** (egret.N) vào capability matrix để các capability StarRocks phụ thuộc patch fork (mask/row-filter enforce qua mọi protocol) bị gate theo image đang chạy; EP nhiều engine → lấy engine **ít năng lực nhất** (fail-closed); đổi tên "profile"→"enforcement point" ở **bề mặt** (không đổi bảng/field API).

**Cách làm:** matrix (`egret-perm-ir/src/matrix.rs`) đã key theo `(EpKind, Ranger plugin version)` và `support()` đã hỏi mọi bound engine. SP3a thêm key orthogonal = egret image level, parse từ image tag engine, mang trên `BoundEngine`/`EpInput`, tra cho các row mask/row-filter của StarRocks. Mixed-engine đổi từ "disagree→not measured" thành "most restrictive wins".

## Ràng buộc chung
- Trong SP3 spec + concepts umbrella.
- **Byte-identical gate:** Ranger JSON trên corpus + demo stack KHÔNG đổi sau gói này (mọi engine đó chạy image fork ≥ mức bật).
- egret image level **orthogonal** với Ranger plugin version — không gộp.
- Fail closed: dưới mức bật → gated capability lower to deny.
- Giữ bảng `policy.authz_services`; KHÔNG đổi tên bảng, KHÔNG đổi field API; chỉ đổi text UI.
- Commit co-author Claude Opus 5.5.

## Task 1: Parse egret image level từ tag
`egret-engine-mgmt/src/delegation_default.rs`: thêm `pub fn starrocks_egret_image_level(image: Option<&str>) -> Option<u32>` (lấy `N` trong `…-egret.N`, bỏ digest `@sha256:`, lấy tag sau `:` cuối). Test parse các case (fe:4.1.7-egret.4→4; allin1 …-egret.5@sha…→5; official→None; notanumber→None; None→None). Mirror cách `starrocks_image_is_pre_3_5` tách tag.

## Task 2: Mang image level trên EP input của IR
`ep.rs`: thêm `BoundEngine.egret_image_level: Option<u32>` + `EpInput.default_image_level: Option<u32>`. `ir_load.rs::bound_engines` (SELECT đã có `spec`): parse image tag engine (tái dùng cách lấy `sr_image` từ `spec`, grep `sr_image` để tìm accessor; nếu chưa có thì thêm getter 1 dòng đọc tag từ `spec`) → set field qua `starrocks_egret_image_level`; `default_image_level = None`. Sửa mọi constructor khác (fixtures) thêm `None`. Build 2 crate.

## Task 3: Gate mask/row-filter StarRocks theo image level; mixed = least-capable
`matrix.rs`:
- Const `STARROCKS_MASK_MIN_EGRET_IMAGE: u32 = 3` (patch 0002/0003/0004 đủ — **verify theo egret-starrocks/README.md + patches/**; default egret.4 thỏa).
- `Entry` thêm `min_egret_image: Option<u32>` (None = không gate). Các row StarRocks mask/row-filter (`ClassificationSelector(Mask|RowFilter)` + `MaskTypes(STARROCKS_MASK_TYPES)`) = `Some(const)`; còn lại `None`.
- `support_for_kind` thêm param `image_level: Option<u32>` (sau `version`): sau khi match `Entry`, nếu `min_egret_image=Some(min)` và `image_level < min` (hoặc None) → `Missing(NotMeasured)`.
- `support(ep,cap)`: collect `(version, image_level)` mỗi engine (hoặc defaults khi rỗng), chọn **restrictive nhất** (Native<Lowered<Missing; cùng tier mà khác → `VersionsDisagree`).
- Sửa 2 call site `enforcement.rs:59/153` (truyền image level; :153 là channel probe → `None`).
- Test: mask cần image ≥ mức bật (≥→ExpandPerColumn, <0/None→Missing); non-mask row bỏ qua image level; mixed-engine (1 engine image 0) → Missing. Chạy `cargo test -p egret-perm-ir` (cập nhật test cũ gọi `support_for_kind` thêm arg; mask test truyền `Some(const)`).

## Task 4: Chứng minh byte-identical trên corpus + demo
Tìm test byte-identical/golden của SP1 (`grep byte-identical|golden|bless` trong `egret-perm-compile/tests`), chạy → KHÔNG diff (mọi engine corpus/demo ≥ mức bật). Nếu diff: fixture corpus thiếu image level → set fixture (trong scope); KHÔNG hạ const để ép pass.

## Task 5: Rename profile→enforcement point (bề mặt)
Grep text UI (`frontend/src`, loại user profile/profile picture) + message/description Rust nói "profile" nghĩa authz profile. Đổi **display string** "profile"→"enforcement point". KHÔNG đổi: tên bảng, field struct/TS, JSON key API (`profile_id`…), route, test id. Build FE + typecheck. Live check screenshot trang governance/engines (lưu dưới data-portal/).

## Ngoài phạm vi
SP3b (Migrate bỏ manual policy); SP3c (clearance→withhold, role→groups, implicit→explicit); SP3d (gate + switch demo); SP5 (vocab đầy đủ, gộp catalog, iceberg-rest). Không đổi tên bảng/field API. `default_image_level` để `None` tới khi gói sau cần.
