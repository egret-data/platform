# StarRocks — Ngang hàng Cấu hình Engine — Thiết kế

**Ngày:** 2026-07-16 · **Trạng thái:** Thiết kế — chờ duyệt · **Nhánh:** `feat/starrocks-config-parity` (off `main`)
**Chương trình:** Nối tiếp StarRocks-ngang-hàng-Trino. Ba lát delegation/cấp phát (#259/#261/#263) đã ship phần *tạo* engine StarRocks; phần này đưa **bề mặt quản lý sau khi tạo** (tab Configuration + Security, sửa + restart) ngang với trang engine-detail của Trino.

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-16-starrocks-config-parity-design.md`.

## Vấn đề

Sau khi tạo, trang chi tiết engine StarRocks (`EngineDetail.tsx`) chỉ có Overview / Catalogs / Workspaces / Monitor. Tab **Configuration** và **Security & Exposure** bị ẩn (`!isStarRocks`), Restart bị vô hiệu, vì: `update_engine` (`engines.rs:2604`) + `restart_engine` (`:2729`) **fail-fast 400** với engine không phải Trino (`build_provision_req` chỉ dựng `EngineSpec` Trino; restart StarRocks là no-op); và form config Trino (`useEngineConfigForm`) hoàn toàn theo hình dạng Trino. ⇒ StarRocks chỉ tạo-rồi-xoá, không sửa image/scale/delegation/restart mà không dựng lại cluster (mất catalog + assignment).

## Mục tiêu

Cho engine StarRocks **cùng bộ khung quản lý** như Trino — tab Configuration + Security **lưu và re-provision**, cùng Restart hoạt động — với **các trường phù hợp StarRocks** (bề mặt config hai engine gần như không trùng, nên đây là ngang-hàng-khung-UX chứ không copy từng trường).

**Sửa được (Configuration):** FE image, CN image, FE replicas, CN replicas, CPU min/max, memory. **Chỉ đọc:** storage shared-data (endpoint/path) — đặt lúc tạo; đổi sẽ mồ côi storage volume.
**Sửa được (Security):** chế độ delegation (None / JWT / Impersonate), TLS bật/tắt, ràng buộc Ranger.
**Restart:** roll pod FE + CN.

Chứng minh live: sửa replicas/resources/image → cluster re-provision; đổi delegation → bootstrap chạy lại + workspace đã gán nhận chế độ mới; restart → FE/CN roll.

## Ngoài phạm vi

- **Không thêm section kiểu-Trino cho StarRocks** (JVM heap, resource groups, autoscaling, ingress, password-file) — đó là khái niệm Trino.
- **Không di trú storage** — storage shared-data giữ create-time (read-only).
- **Cơ chế delegation KHÔNG đổi** — tái dùng builder lát-3 `ensure_tls_cert`/`ensure_delegator_secret`/`StarRocksDelegation` + bootstrap `spawn_starrocks_catalog_wire`; chỉ làm chúng gọi được từ `update`, idempotent.
- **RisingWave: ngoài.**

## Bối cảnh (đã kiểm chứng)

- **`StarRocksProvisioner::update` ĐÃ CÓ** (`orchestrator/starrocks.rs:372`), re-apply CR bằng server-side apply. Lớp provisioner sẵn sàng; nghẽn ở *handler*.
- **`EngineProvisioningPort`** có provision/status/update/delete/restart; dispatch theo `kind` qua `for_kind` (`main.rs:487`) — nhưng `restart` cho `"starrocks"` trả `Ok(())` stub (`main.rs:525`), operator sở hữu StatefulSet FE/CN (patch trực tiếp bị revert).
- **`create_starrocks_engine`** dựng `spec` + `StarRocksDelegation` từ body, mint cert TLS + delegator secret, persist, provision, `spawn_starrocks_catalog_wire`. Update phải tái dùng đường dựng-spec này (tách helper `build_starrocks_spec`).
- **Materialize assignment** (`assignment.rs`) chụp `impersonate`/`tls_cert` vào hàng `databases` mỗi workspace lúc gán. Đổi delegation phải chạy lại materialize cho các workspace đã gán (quyết định đã duyệt).

## Kiến trúc

**A. Backend `update_engine` nhánh StarRocks:** tách `build_starrocks_spec(body, existing)` khỏi create (create/update sinh spec giống hệt; storage đọc từ spec *cũ*). `update_engine` thay 400 bằng `match type`; StarRocks → rebuild spec → persist → `build_provision_req` → `prov.update`. Tái dùng `ensure_tls_cert`/`ensure_delegator_secret` (idempotent). Nếu delegation đổi → chạy lại `spawn_starrocks_catalog_wire`. **Re-materialize** kết nối cho mỗi workspace đã gán (tách `rematerialize_delegated_conn` dùng chung với `assign`).

**B. `build_provision_req` nhánh StarRocks** (`engines.rs:511`): thêm arm StarRocks dựng `EngineSpec` StarRocks (giống create). Arm Trino giữ nguyên.

**C. `restart_engine` StarRocks** (`engines.rs:2729` + `main.rs:524`): thay 400 + stub `Ok(())`. Restart = re-apply CR với annotation pod-template `egret.io/restartedAt` mới → operator roll StatefulSet FE/CN, egret không đụng object operator-owned.

**D. FE `useStarRocksConfigForm`:** hook form hình-dạng-StarRocks (song song `useEngineConfigForm`): feImage/cnImage/feReplicas/cnReplicas/cpuMin/cpuMax/memory/delegationMode/tlsEnabled + Ranger. Seed từ `engine.spec.starrocksConfig`, `dirty`, `save()` PUT qua `updateEngine`.

**E. FE bỏ gate `EngineDetail.tsx`:** render tab Configuration + Security StarRocks khi `isStarRocks`; bỏ alert "not yet supported"; bật Restart. Config tab: nav Image/Compute/Resources (+ Storage read-only). Security tab: Delegation + TLS + Ranger.

## Luồng dữ liệu

- **Sửa config:** Configuration → `save()` → `PUT /engines/{id}` → `update_engine` rebuild spec + `prov.update` → operator roll role đã đổi.
- **Đổi delegation:** Security → `PUT` → re-mint (idempotent) + bootstrap lại + **re-materialize kết nối mỗi workspace đã gán** → query dùng chế độ mới ngay.
- **Restart:** nút Restart → `POST /engines/{id}/restart` → nhánh StarRocks re-apply CR với `restartedAt` mới → FE/CN roll.

## Kiểm thử

- **Unit (CI):** `build_starrocks_spec` (giữ storage từ existing); arm StarRocks của `build_provision_req`; detector đổi-delegation; `rematerialize_delegated_conn` đặt đúng `impersonate`/`tls_cert`. FE: `useStarRocksConfigForm` seed + dựng body PUT; tab render + submit; `EngineDetail` hiện tab cho engine StarRocks.
- **Live:** trên StarRocks thật — (1) FE replicas 1→2 + đổi memory → CR re-apply, pod roll; (2) delegation None→Impersonate → bootstrap lại, query workspace đã gán đổi identity; (3) Restart → FE/CN roll.

## Rủi ro chỉ-live

1. Ngữ nghĩa re-apply CR (operator có roll StatefulSet khi đổi replicas/resources/image?).
2. Annotation restart có thực sự roll StatefulSet qua operator không.
3. Thời điểm re-materialize khi có query đang chạy (chấp nhận: query kế tiếp nhận chế độ mới).

## Phân rã (cho plan)

1. Refactor `build_starrocks_spec`. 2. `update_engine` StarRocks (image/replicas/resources). 3. Đổi delegation khi update (re-mint + bootstrap + re-materialize). 4. `restart_engine` StarRocks. 5. `useStarRocksConfigForm` + Configuration tab. 6. Security tab + bật Restart. 7. Live smoke.
