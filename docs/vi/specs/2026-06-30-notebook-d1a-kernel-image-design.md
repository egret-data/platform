# Notebook D1a — Kernel Image + Helpers + Query Callback (thiết kế)

**Trạng thái:** nháp · **Ngày:** 2026-06-30 · **Epic:** Notebooks · **Trước đó:** F1 (#95), D1b (#96), D1c (#97) — đã merge
> Bản tiếng Anh là canonical: `docs/en/specs/2026-06-30-notebook-d1a-kernel-image-design.md`. Bản này tham chiếu.

## 1. Bối cảnh & phạm vi
D1 = **D1a (image+helpers+query-callback, spec này)** · D1b (launcher+lifecycle, XONG) · D1c (WS proxy, XONG). D1b spawn Jupyter Kernel Gateway per (user×workspace) + inject manifest F1 vào env (`EGRET_NOTEBOOK_RUNTIME`/`EGRET_API_URL`/`EGRET_KERNEL_TOKEN`). D1c proxy channels WS. **D1a làm kernel thực sự dùng được:** image (Daft preload), helper Python (`query()`/`catalog()`/`display()`), và endpoint callback + kernel token thật mà `query()` cần. Sau D1a: cell notebook đọc data có kiểm soát qua `query()` (Trino/StarRocks+Ranger, theo danh tính user) + đọc/ghi namespace Iceberg riêng qua `catalog()` (Daft+pyiceberg). **E1** sau nối cell-type UI + render output dùng các helper này — D1a là lớp năng lực, không phải UX.

### Trong phạm vi
- **Kernel image** (`kernel-image/`): Jupyter Kernel Gateway + Python 3.11 + Daft + pyiceberg + helper package; `entrypoint.sh` chạy gateway :8888.
- **Helper package** (`egret_kernel_helpers`, pip cài được): `query()`/`catalog()`/`display()`/`runtime`/`me` — auto-load vào mọi kernel qua IPython startup shim (không cần import).
- **Query callback** (Rust): `POST /api/kernel/query` — auth bằng token, chạy SQL trên engine của workspace (đã enabled) với Ranger impersonation (`X-Trino-User`), trả JSON rows.
- **`EGRET_KERNEL_TOKEN`** (Rust): bearer ngắn hạn thật, mint ở nhánh spawn, hash vào row `kernels` (migration append-only), inject vào env kernel, revoke khi stop/reap. Thay placeholder `""` của D1b.

### Ngoài phạm vi (sau)
Nối cell execute + render output + nút Run = **E1** (4 loại cell + render). PySpark/Spark Connect (D1a chỉ Daft). Launcher docker/k8s (D1b chỉ có subprocess; image là môi trường chuẩn cho các mode đó; host subprocess pip-cài cùng helper package). Arrow transport (MVP dùng JSON columns+rows, tái dùng coercion sẵn có; Arrow tối ưu sau).

## 2. Quyết định (brainstorm)
- **`query()` = callback về backend, egret chạy SQL server-side** (chọn thay vì kernel tự nối JDBC, hay hoãn sang E1). Kernel POST `{engine, sql}` → egret chạy trên Trino/StarRocks với `X-Trino-User` Ranger → trả rows. Kernel **không** giữ credential, **không** cần driver — egret là choke-point governance duy nhất. Cái giá: kernel token thật thay placeholder.
- **Chỉ Daft + pyiceberg** trong image (không PySpark).
- **Verify: pytest (helpers) + CI `docker build` + docker-run smoke** (chạy container, kiểm gateway sống, chạy `1+1` qua nó). Governed-read end-to-end thật (Trino+Ranger) = thủ công, documented.
- **Hai plan build sau hợp đồng query:** P1 = Rust (token + callback), P2 = Docker/Python (image + helpers). Test độc lập.

## 3. Components
- **3.1 `EGRET_KERNEL_TOKEN` (egret-kernel):** bearer ngắn hạn để kernel gọi lại egret. **Chỉ mint ở nhánh spawn** trong `ensure_running` (KHÔNG ở nhánh reuse — reuse giữ token cũ). Lưu **hash (SHA-256)** trên row `kernels` qua migration `0043` (thêm `token_hash`, `token_expires_at`), giống pattern `catalog_tokens`/session. Plaintext sinh 1 lần, `ensure_running` chèn vào env map (nên `manifest_to_env` bỏ set placeholder), không lưu plaintext. Revoke ngầm khi xóa row lúc stop/reap. Validator hash bearer → match row sống chưa hết hạn → ra `(user_id, workspace_id)`; username từ `UserRepo::ensure_username` (F1).
- **3.2 Query callback:** `POST /api/kernel/query` (token plane, không session cookie). Body `{engine, sql, max_rows?}`. Luồng: (1) validate token → user+ws; (2) resolve `engine` theo enabled-notebook-engines của ws (unknown/disabled → 403); (3) chạy SQL qua connector egret-engine cho database đó, set `X-Trino-User=<username>` (đường impersonation F1 mà BI/sqllab đang dùng), cap `max_rows`; (4) trả `{columns, rows}` JSON, tái dùng coercion theo cột sẵn có. Lỗi: 401 (token sai/hết hạn), 403 (engine chưa enabled/cross-ws), 400 (lỗi SQL/engine). KHÔNG bao giờ chạy SQL trên engine ws chưa enabled cho notebook.
- **3.3 Kernel image:** `FROM python:3.11-slim`; `pip install jupyter_kernel_gateway daft pyiceberg requests` + helper package; `entrypoint.sh` chạy `jupyter kernelgateway --ip=0.0.0.0 --port=8888 --auth_token='' --list_kernels=True` (khớp D1b spawn/D1c proxy). Helper package có IPython startup file (cài vào `startup/`) import helper vào namespace → `query`/`catalog`/`display` dùng không cần import. Image đa dụng: mode docker/k8s chạy trực tiếp; host SubprocessLauncher (MVP D1b) pip-cài cùng package + Daft.
- **3.4 Helpers (`egret_kernel_helpers`):** load 1 lần lúc start; đọc `EGRET_NOTEBOOK_RUNTIME` (parse thành `runtime`), `EGRET_API_URL`, `EGRET_KERNEL_TOKEN`.
  - `query(engine, sql, max_rows=10000) -> daft.DataFrame`: resolve `engine` theo id/name trong `runtime.engines`; POST callback (Bearer token); parse `{columns, rows}` → `daft.from_pydict`. 401/403/timeout → lỗi rõ, không trả frame rỗng âm thầm.
  - `catalog(id=None)`: **tự đủ** (không callback): dựng pyiceberg `RestCatalog` từ `runtime.catalog` (`uri`/`warehouse`/`token`); namespace mặc định = `notebook_{username}`. Helper list/đọc table thành Daft (`daft.read_iceberg`) + ghi Daft DataFrame về namespace riêng. `id`/name chọn catalog ws đã enabled khi truyền.
  - `display(obj, n=50)`: Daft DF có sẵn `_repr_html_`; `display` cap row + render, non-DataFrame thì fall through IPython.
  - `runtime` (manifest đã parse) + `me` (= username).

## 4. Luồng dữ liệu
- **Governed read:** cell `query("trino-dev", sql)` → `POST /api/kernel/query` (Bearer token) → egret resolve user+ws, chạy SQL Trino/StarRocks với `X-Trino-User=<username>` (Ranger chặn) → JSON `{columns, rows}` → `daft.from_pydict`.
- **Private rw:** cell `catalog().load_table("notebook_alice.t")` → pyiceberg `RestCatalog` (token 1-grant) → Iceberg REST `notebook_{user}` → Daft đọc/ghi thẳng (egret không trong data path).

## 5. Auth / bảo mật
Token scope 1 `(user_id, workspace_id)`, hash khi lưu, ngắn hạn (= đời kernel), revoke khi stop/reap — lộ token chỉ cấp đúng quyền user đã có, đến khi kernel chết. Callback chỉ chạy SQL trên engine ws đã enabled cho notebook, LUÔN as end-user qua `X-Trino-User` (không as service principal) — Ranger là cổng policy. Plane catalog giữ như F1 (token 1-grant scope namespace riêng). Container không giữ credential dài hạn nào ngoài token kernel ngắn hạn + token catalog 1-grant.

## 6. Xử lý lỗi
`query()`: 401 → "kernel token hết hạn (restart kernel)"; 403 → "engine chưa enabled / không có quyền"; lỗi SQL/engine → 400 kèm message; timeout/unreachable → RuntimeError rõ. `catalog()`: thiếu warehouse/token → lỗi rõ trỏ về config catalog ws. Token hết hạn/revoke/unknown → callback 401 (kernel chạy lại `connect` → kernel mới → token mới). Image: thiếu env `EGRET_NOTEBOOK_RUNTIME` → helper load degraded, gọi mới báo lỗi rõ (chạy `jupyter` trần không crash lúc import).

## 7. Kiểm thử
- **pytest helper package** (không kernel, không network): mock env + mock callback HTTP → `query()` dựng request đúng + biến `{columns, rows}` thành Daft DF đúng; resolve engine name→id; `catalog()` dựng `RestCatalog` kwargs đúng (mock pyiceberg); `display()` render DF→HTML + fall through; 401/403/timeout báo lỗi rõ.
- **Rust callback** (`#[sqlx::test]` + fake engine connector, không Trino thật): validate token (hợp lệ→ra user; sai/hết hạn→401); gate engine-enabled (enabled→chạy; disabled/cross-ws→403); set `X-Trino-User`; shape JSON `{columns, rows}`; mint-on-spawn / không-on-reuse / revoke-on-stop.
- **CI:** `docker build` image; **docker-run smoke** — chạy container, `GET /api/kernels` (gateway sống), start kernel python3 + chạy `1+1` qua gateway REST, assert `2`. (Không Trino/Ranger trong CI.)
- **Thủ công (documented):** governed-read end-to-end thật (Trino + Ranger + warehouse provisioned).
- **CI local trước merge; migration append-only.**

## 8. Phân rã build (2 plan sau hợp đồng query)
- **P1 — Rust:** migration 0043 (`kernels.token_hash`/`token_expires_at`) · mint token (nhánh spawn) / validate / revoke · extractor `require_kernel_token` · `POST /api/kernel/query` (gate engine-enabled + chạy `X-Trino-User` + JSON) · wire egret-kernel/egret-server/egret-bin.
- **P2 — Docker/Python:** `kernel-image/` Dockerfile + entrypoint · package `egret_kernel_helpers` (`query`/`catalog`/`display`/`runtime`/`me`) + IPython startup shim · pytest · CI `docker build` + docker-run smoke.

## 9. Câu hỏi mở (chốt khi plan)
1. Crate cho query callback: mở rộng `egret-kernel` + dep `egret-engine`, hay đặt endpoint ở `egret-server`. *Lean: tránh dependency cycle — handler mỏng trong egret-kernel nhận 1 trait engine-runner, runner cụ thể inject từ egret-bin (giống pattern launcher/proxy).*
2. Tái dùng coercion kết quả engine: xác nhận hàm/shape tái dùng từ đường BI/sqllab để callback trả `{columns, rows}` y hệt. *Lean: tái dùng type kết quả engine sẵn có, không tự chế coercion.*
3. Cơ chế auto-load helper: file IPython `startup/` do image copy vs entry-point/`.pth`. *Lean: startup file do package cài (test được + chạy dưới IPython profile của gateway).*
