# Notebook E — Wiring thực thi (thiết kế · bản tham chiếu)

> Bản chuẩn: `docs/en/specs/2026-07-01-notebook-e-execution-design.md`. File này là bản tham chiếu rút gọn.
> Nền: D1a/D1b/D1c (đã merge) + F2 OIDC delegation (đã merge → engine SQL chạy governed per-user). Trạng thái: nháp để review · 2026-07-01.

## Mục tiêu
Làm cho cell notebook **chạy được**. Nối execution end-to-end trên kernel Python/Daft per-(user×workspace): trình duyệt điều khiển kernel, cell thực thi, output render + lưu. Đây là sub-spec cuối của epic Notebook (A→B→C→D→**E**).

## Kiến trúc (Option A — FE là Jupyter client qua proxy D1c)
Trình duyệt = **Jupyter client** nói qua **1 WS multiplex** đã được D1c proxy (`/notebooks/{nb}/kernel/channels`, msg có field `channel`). Proxy backend **tap** chiều iopub (kernel→FE) để **lưu output (recorder backstop)**. FE render live; **recorder sở hữu persistence** (FE KHÔNG PATCH output → tránh race dual-write của sparklabx). Route iopub theo `parent_header.msg_id`.

## Thành phần
1. **FE kernel client** (`frontend/src/lib/kernel/`): mở WS sau `/connect` ready; framing Jupyter (header/parent_header, channel, session, msg_id, v5.3); `execute(cellId, code)` gắn `metadata.egret_cell_id`; route iopub (status/stream/execute_result/display_data/error/execute_reply); reconnect.
2. **SQL cell-magic** (`kernel-image/.../magic.py`): `%%egret_sql --target daft|<engine>` — daft→`daft.sql()` trên Iceberg catalog đã attach; engine→helper `query()` (callback D1a `POST /kernel/query`, **post-F2 trình JWT user tới Trino/StarRocks → authz per-user thật**). Đăng ký trong startup shim D1a. → kernel image lên version, mở rộng pytest D1a.
3. **Cell model + migration**: thêm `sql_target text NULL` (mặc định 'daft'); PATCH cell nhận nó. Contract `last_output` jsonb = mảng output item (stream/execute_result/display_data/error), **cap ~256KB server-side** (recorder truncate). Không S3 spill (spec B chưa có).
4. **Recorder + controls backend** (`egret-kernel`): mở rộng tap `BackendWsProxy` parse iopub, gom theo `egret_cell_id`, lưu `last_output`/`execution_count`/time qua seam `NotebookOutputStore` (bền cả khi đóng tab). `GET .../kernel/active-executions` restore in-flight. `POST .../kernel/interrupt|restart` (passthrough REST tới Kernel Gateway).
5. **FE notebook UX** (`NotebookPage`/`CellEditor`): status bar kernel; bật Run + state per-cell; dropdown target cho cell SQL (từ manifest F1 `engines`); **outputs renderer** (port sparklabx `CellOutputRenderer`+`ansiToHtml`): text/html (DOMPurify), image/png+jpeg, text/plain, traceback ANSI→HTML, error boundary. Toolbar full: Run cell/all (**queue-upfront + stop_on_error**)/above/below, Interrupt, Restart, Clear.

## Luồng chạy 1 cell
Run → ensure kernel + WS → (SQL: prepend `%%egret_sql --target <t>`; code: raw; markdown: không chạy) → `execute_request` (+ `metadata.egret_cell_id`), cell=running → iopub qua proxy → FE render theo msg_id; recorder tap gom+lưu → idle: cell done, recorder ghi `last_output` (capped)+count+elapsed. Lỗi → render traceback + dừng run-all.

## Test
- pytest (mở rộng D1a): magic `%%egret_sql` (daft+engine, parse/escape) với daft/query fake.
- Backend: recorder parse+persist+cap (msg giả), active-executions, interrupt/restart (fake gateway), PATCH `sql_target`.
- FE Vitest: client routing (parent_header), renderer từng MIME + ANSI, run-all queue/stop-on-error, restore-on-load.
- Live smoke (thủ công): Python/Daft, SQL/Daft, SQL/engine (governed per-user qua F2), plot png, lỗi, đóng-tab-giữa-chừng→mở lại thấy output (kiểm recorder).

## Phase (1 spec)
- **E-P1**: magic + migration `sql_target` + interrupt/restart + contract output & cap.
- **E-P2**: recorder/tap + `NotebookOutputStore` + active-executions.
- **E-P3**: FE client + renderer + run-single + restore-on-load + status bar.
- **E-P4**: full controls (run-all upfront/above/below, interrupt, restart, clear) + dropdown SQL target.

## Ngoài phạm vi
S3 output spill (cần spec B); MIME phong phú hơn png/html/plain; RTC/collab; chọn kernel per-cell (đã là single Python kernel — C1 reshape).
