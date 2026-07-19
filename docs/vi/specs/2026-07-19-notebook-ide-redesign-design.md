# Thiết kế lại Notebook theo hướng IDE — Thiết kế

- **Ngày:** 2026-07-19
- **Trạng thái:** Đã duyệt (brainstorm) → sẵn sàng lập plan triển khai
- **Phạm vi:** `egret/frontend/src/**/notebook*`, `egret/frontend/src/lib/kernel/**`, `egret/crates/egret-notebook`, `egret/kernel-image`
- **Liên quan:** notebook epic (A–E đã xong), F2 OIDC delegation, mô hình quản trị ba tầng. Sản phẩm tham khảo: UX notebook của `sparklabx`.

> Bản tiếng Anh là canonical: `docs/en/specs/2026-07-19-notebook-ide-redesign-design.md`. Tài liệu này là bản tham chiếu tiếng Việt.

## 1. Vấn đề & Mục tiêu

Notebook của egret đã chạy end-to-end (cell chạy trên kernel Daft/Jupyter thật, output được lưu, có nút điều khiển chạy) nhưng UI tối giản: trang một-cột (`NotebookPage.tsx`, ~470 dòng), output renderer 38 dòng, không sidebar, không chỉ báo trạng thái kernel, không bộ đếm thực thi, không có SQL cell hạng nhất. So với Hex / Databricks / notebook sparklabx trau chuốt thì trông "sơ khai".

**Mục tiêu:** nâng *trải nghiệm biên tập* notebook lên mức IDE, cảm giác giống Hex, trong khi **giữ nguyên** các khác biệt kiến trúc của egret (truy cập 2-plane có governance, catalog Iceberg per-workspace, Ranger/OIDC delegation). Đây là nỗ lực **UX + một lát cắt dọc mỏng**, **không** đổi mô hình thực thi (vẫn tuần tự, một kernel Daft — không reactive DAG, không cluster scaling).

### Không làm (hoãn có chủ đích)
- Reactive/DAG execution, cộng tác real-time, lịch sử phiên bản notebook.
- Resource-usage badge (cần endpoint backend `/kernel/usage`; SubprocessLauncher không có metric container).
- File browser cá nhân (Spec B — chưa có backend).
- Publish-to-BI từ notebook (spec riêng; ghi nhận là giá trị cao trong tương lai).

## 2. Hiện trạng (đã kiểm chứng 2026-07-19)

- **Mô hình kernel là DataFrame-first.** `egret_kernel_helpers` cung cấp `trino(sql)` / `starrocks(sql)` → **Daft DataFrame** (qua `daft.read_sql` với JWT per-user tươi, có governance), `write_table(df, …)`, và `display(obj)` (render Daft DataFrame giới hạn 50 dòng). **Không còn** magic `%%egret_sql` (chỉ còn `.pyc` cũ); đường SQL-cell cũ đã bị bỏ.
- **Cột `sql_target` từng được thêm (migration 0046) rồi bị gỡ (0047).** `notebook_cells` hiện không có cột SQL. FE `CellType` vẫn liệt kê `"sql"` nhưng không có gì hậu thuẫn; `CellEditor` render mọi cell non-markdown như Python.
- **`KernelConnection`** có `KernelStatus` (`idle|busy|spawning|connected|disconnected`) nhưng chỉ phát `onStatus(busy|idle)` theo từng lần execute — **không** có luồng trạng thái kết nối reactive.
- **`useUpdateCell` / `useAddCell`** chỉ truyền `cell_type` + `source` (không có field SQL).
- **`CellOutputRenderer`** (egret, 38 dòng) render một phần; `ansiToHtml.ts` đã port sẵn. Renderer 395 dòng của sparklabx có error boundary, cache ANSI/split đã memo, HTML(DOMPurify)/png/jpeg/plain/error+traceback và logs collapse — nhưng bộ lọc noise của nó là Spark/Scala-specific và nút "Debug AI" cần AIContext mà egret không có.
- Token thiết kế egret là **OKLCH** (`src/index.css`); các token ngữ nghĩa liên quan có sẵn: `--primary` (teal), `--success` (xanh lá), `--destructive`, `--muted-foreground`, `--card`, `--border`, cả light + dark. **Không thêm màu hay font mới.**

## 3. Ngôn ngữ thiết kế

- **Style:** "Soft UI Evolution" điều chỉnh theo token egret — thẻ `bg-card border border-border rounded-lg shadow-sm`; cell đang chạy có `ring-1 ring-primary/40`. Bố cục dày (mật độ công cụ dev): `gap-2/3`, toolbar `py-1`.
- **Ánh xạ màu (ngữ nghĩa, không hex thô):** connected/OK → `--success`; connecting/busy/restarting → `--primary` (busy pulse nhẹ); error/dead → `--destructive`; disconnected → `--muted-foreground`. Trạng thái **không bao giờ chỉ dựa màu** — luôn kèm nhãn chữ + tooltip.
- **Chữ:** code + output dùng `font-mono`; Monaco lo font trong editor. Không import font.
- **Motion:** 150–200ms cho hover / collapse / toggle sidebar; bọc trong `prefers-reduced-motion`.
- **A11y:** aria-label cho nút icon-only, focus ring rõ, heading tuần tự, output error có icon + chữ, HTML DataFrame sanitize bằng DOMPurify.

## 4. Kiến trúc — bản đồ component

Đều dưới `egret/frontend/src`. Mới = ✚, sửa = ✎.

```
routes/explore/NotebookPage.tsx            ✎ tái cấu trúc thành IDE shell 3 vùng
components/notebook/
  NotebookShell.tsx                        ✚ layout 3 vùng (rail | panel | main)
  NotebookHeader.tsx                       ✚ breadcrumb, tên, public, kernel badge, run toolbar
  SidebarIconRail.tsx                      ✚ tab: TOC · Data Sources (collapse được)
  panels/TableOfContentsPanel.tsx          ✚ heading/cell → cuộn tới
  panels/DataSourcesPanel.tsx              ✚ engines + catalogs từ runtime (read-only)
  KernelStatusBadge.tsx                    ✚ dot reactive + nhãn + tooltip
  CellEditor.tsx                           ✎ gutter In[N], accent bar, sql target/into, md toggle
  CellOutputRenderer.tsx                   ✎ port (rút gọn) từ sparklabx
  ansiToHtml.ts                            (dùng lại)
  monaco/registerStatic.ts                 ✚ static completions python + sql
lib/kernel/
  KernelConnection.ts                      ✎ thêm status emitter + status interrupt/restart
  useKernelStatus.ts                       ✚ subscribe trạng thái kết nối
  protocol.ts                              (dùng lại union CellOutput)
api/notebooks.ts                           ✎ sql_target + sql_into trên Cell + mutations
```

### 4.1 IDE shell (`NotebookShell` + `NotebookPage` tái cấu trúc)
Ba vùng:
1. **`SidebarIconRail`** (`w-12`, collapse): hai tab — **Table of Contents**, **Data Sources**. Tab active highlight (`bg-primary/10 text-primary`). Bấm tab đang active thì thu panel. (Cố tình bỏ các mục "Notebooks"/"My Files" trong rail của sparklabx: điều hướng notebook đã có ở app sidebar, và không có backend file cá nhân.)
2. **Panel** (`w-64`, ẩn khi collapse): render panel đang chọn.
3. **Main**: `NotebookHeader` sticky + danh sách cell cuộn được + affordance thêm cell. Header `sticky top-0 z-10` để nút chạy luôn hiện trong notebook dài.

Layout xuống dưới `md` co giãn mượt: rail thành toggle trên đỉnh, panel thành overlay sheet. Body trang không cuộn ngang; output rộng cuộn trong container riêng.

### 4.2 `NotebookHeader`
Breadcrumb `← Notebooks` · tên (rename-on-blur, logic sẵn có) · Public `Switch` (sẵn có) · **`KernelStatusBadge`** · run toolbar: **Run all · Interrupt · Restart · Clear outputs** (dùng lại runner-registry sẵn có trong `NotebookPage`; icon Lucide: `Play`, `Square`, `RotateCcw`, `Eraser`). Nút disable hợp lý (Run-all khi đang chạy; Interrupt chỉ khi busy).

### 4.3 `KernelStatusBadge` + luồng trạng thái `KernelConnection`
`KernelConnection` thêm trạng thái observable nhẹ:
- Nội bộ `status: KernelStatus` + `listeners: Set<(s)=>void>`, `onStatusChange(cb): () => void`, `setStatus(s)` private.
- Phát chuyển trạng thái: `disconnected` → `connecting` (khi `connect()` bắt đầu) → `connected` (ws mở + kernel ready) → `busy`/`idle` (từ iopub status, đã parse sẵn) → `disconnected` (khi `disconnect()`/close). `interrupt()`/`restart()` đặt `interrupting`/`restarting` tạm; lỗi → `error`; close code kernel-chết → `dead`.
- Mở rộng union `KernelStatus` thêm `connecting | interrupting | restarting | error | dead`.

`useKernelStatus(kernel)` subscribe và trả trạng thái hiện tại cho badge. Badge = dot màu (ánh xạ token §3) + nhãn fixed-width + tooltip `title`; `aria-live="polite"` trên nhãn để screen reader nghe được chuyển trạng thái.

### 4.4 `CellEditor`
- **Gutter trái:** prompt `In [N]:` (execution count từ `cell.execution_count`), hoặc spinner khi chạy; **thanh accent** mảnh đổi màu theo trạng thái (đang chạy → `--primary`, lần chạy trước lỗi → `--destructive`, ok → `--success`, idle → `--border`).
- **Type select** thêm **SQL** (`code | markdown | sql`). Monaco `language` theo type (`python` / `sql`); markdown dùng textarea + preview sẵn có, nay là **toggle edit/preview** (preview khi cell không focus, giống sparklabx).
- **Khung SQL cell:** **dropdown target** (options từ `useNotebookRuntime().engines`, default = engine mặc định của workspace; "Daft (local)" là option stretch) bind `sql_target`, và ô nhỏ **"→ biến"** bind `sql_into` (default `df`). Xem §5.
- **Toolbar cell** (move/delete/run + dropdown run-above/below sẵn có) chuyển sang hiện-khi-hover để bớt rối; **Shift+Enter chạy** cell (keybinding Monaco + handler textarea).
- **Output** render qua `CellOutputRenderer`.

### 4.5 `CellOutputRenderer` (port rút gọn)
Port renderer của sparklabx, **bỏ** bộ lọc noise Spark/Scala và nút "Debug" AI:
- `CellOutputErrorBoundary` mỗi cell (giới hạn lỗi render HTML hỏng trong một cell).
- Cache đã memo: `WeakMap` ANSI-split + cache ANSI→HTML có giới hạn (tránh O(n²) khi streaming).
- Render: `stream` (stdout/stderr → `<pre>` ANSI), MIME bundle `result` — `text/html` (DOMPurify, `overflow-auto max-h-96`), `image/png`, `image/jpeg`, `text/plain`, fallback JSON — và `error` (icon + `ename: evalue`, traceback ANSI, style theo `--destructive`).
- **Logs collapse:** một disclosure "Logs (N dòng)" chung, gộp các dòng phân loại `stderr` (thay cho split Spark/Scala/Delta cụ thể). Giữ đơn giản: stdout = output người dùng; stderr = logs collapse được.

### 4.6 Panels
- **`TableOfContentsPanel`:** dựng mục từ cell — heading markdown (parse `#`/`##`) và nhãn gọn cho cell code/SQL (`In [N]` + dòng đầu không trống). Bấm → `scrollIntoView` tới cell (mỗi `ManagedCell` expose ref qua registry sẵn có hoặc anchor `id={cell.id}`). Thụt lề theo cấp heading.
- **`DataSourcesPanel`:** liệt kê `useNotebookRuntime().engines` (đánh dấu default) và catalogs, read-only. Mỗi dòng: icon engine + tên + kind. Footer gợi ý cú pháp copy-paste: `df = trino("SELECT … ")` / `df = starrocks("…")`. Không "add connector" (egret cấu hình ở trang Notebook Settings).

### 4.7 Monaco completions (`monaco/registerStatic.ts`)
Đăng ký static completion một lần: **python** — `trino`, `starrocks`, `write_table`, `display`, `runtime`, `me`, cùng các thao tác Daft phổ biến; **sql** — keyword chuẩn + tên catalog/schema của engine bật (nếu lấy rẻ được, không thì chỉ keyword). Điều chỉnh (không copy nguyên) từ `monaco/` của sparklabx.

## 5. SQL cell + SQL→DataFrame binding (lát cắt dọc mỏng)

**UX:** SQL cell chứa SQL sạch (không thấy magic). Nó có **target** engine và **tên biến** output. Chạy xong, kết quả bind vào một **Daft DataFrame thật** trong namespace kernel, để cell Python phía sau dùng lại (`sales_df.filter(...)`). Giống `--into` của Databricks và SQL cell của Hex, dùng lại helper DataFrame-first sẵn có của egret — **không viết engine query mới**.

### 5.1 Backend (`egret-notebook` + migration)
- **Migration `0048_notebook_cell_sql_binding.sql`** (append-only): thêm lại `sql_target text` và thêm `sql_into text`, cả hai nullable, vào `notebook_cells`. Chạm `egret-metastore/src/lib.rs` để re-embed migrations (gotcha staleness đã biết).
- Truyền `sql_target` + `sql_into` qua model cell, store (create/update), và DTO HTTP (add-cell + update-cell). Giữ nguyên guard IDOR/ownership; không đổi hành vi cell code/markdown (field giữ `NULL`).
- **Regen contract API:** `cargo run -p egret-bin -- openapi > openapi.json` → `npm run gen:api`.

### 5.2 Kernel (`kernel-image`)
- Thêm `egret_kernel_helpers/magic.py` đăng ký cell magic **`%%egret_sql`**:
  - Parse `--target <engine>` (default: engine mặc định) và `--into <var>` (default `df`; validate là identifier Python hợp lệ, không thì raise lỗi rõ).
  - Chạy `trino(sql)` hoặc `starrocks(sql)` (dispatch theo target) → Daft DataFrame.
  - Gán vào namespace người dùng: `get_ipython().user_ns[var] = df`.
  - `display(df)` để cell hiện bảng.
  - Lỗi surface qua `RuntimeError` sẵn có (rõ ràng, không im lặng-rỗng).
- Đăng ký magic trong `startup/00-egret.py`.
- pytest: parse cờ, validate identifier, dispatch, gán namespace (với fake IPython + fake `trino`).

### 5.3 Frontend
- Type `Cell` + `useAddCell`/`useUpdateCell`: thêm `sql_target?: string`, `sql_into?: string`.
- SQL `CellEditor`: dropdown target + ô "→ biến" (persist qua `updateCell`). Để tránh ghi đè âm thầm khi notebook có nhiều SQL cell, FE seed **default không trùng** khi tạo cell: `df` cho SQL cell đầu, rồi `df_2`, `df_3`, … (quét các `sql_into` hiện có). Người dùng đổi tên tự do; tên trùng hiện cảnh báo inline ("dùng lại biến `df` — sẽ ghi đè") thay vì lỗi cứng.
- Khi **Run** một SQL cell, chèn dòng đầu ẩn
  `%%egret_sql --target <sql_target|default> --into <sql_into|df>`
  lên đầu source SQL và gửi cho kernel (editor vẫn hiện SQL sạch — cùng kỹ thuật giấu-magic đã dùng trước). Body là SQL thô, nên không có nguy cơ trích-dẫn chuỗi Python.
- Dưới output, hiện **chip binding**: `sales_df · Daft DataFrame` để người dùng biết tên biến tái dùng.

### 5.4 Stretch — target Daft-local
Target `daft` biên dịch thành `daft.sql("<sql>")` trên các DataFrame đã bind (đăng ký biến đã bind vào catalog SQL của Daft). Hoãn; target v1 là engine.

## 6. Luồng dữ liệu (mô hình thực thi không đổi)
1. FE `KernelConnection.execute(cellId, source)` qua WS proxy D1c — với SQL cell, `source` là `%%egret_sql …\n<sql>`.
2. Kernel magic → `trino()/starrocks()` → JWT per-user → engine có governance (Ranger/OIDC) → Daft DataFrame → bind vào `user_ns[var]` → `display`.
3. iopub → recorder (backend E-P2) lưu `last_output`; FE render live qua `CellOutputRenderer`. FE **không** PATCH output (recorder sở hữu persistence).

## 7. Kiểm thử
- **FE (Vitest):** ánh xạ state→nhãn/màu của `KernelStatusBadge`; subscribe/unsubscribe của `useKernelStatus`; `CellOutputRenderer` cho từng loại MIME + error + logs collapse + giới hạn error-boundary; persistence target/into của SQL `CellEditor` + chèn-magic-khi-run; scroll target của TOC; danh sách DataSources từ runtime mock.
- **Kernel (pytest):** parse cờ `%%egret_sql`, validate identifier, dispatch, bind namespace, display.
- **Backend:** round-trip `#[sqlx::test]` scratch-DB của `sql_target`/`sql_into` khi create/update; NULL cho code/markdown.
- **Live e2e (thủ công, không CI):** theo công thức notebook live-e2e — chạy một SQL cell (`SELECT … `), xác nhận biến đã bind dùng được ở cell Python kế; xác nhận bảng output + bộ đếm In/Out + chuyển trạng thái kernel badge. CI local đầy đủ (fmt/clippy/`cargo test --workspace`, FE lint/typecheck/test/build) xanh trước khi merge.

## 8. Rủi ro / gotcha
- **Regen OpenAPI** phải chạy nếu không DTO FE lệch (`sql_target`/`sql_into`). Giữ convention comment `CellType` bảo trì tay trong `api/notebooks.ts`.
- **Migration staleness:** chạm `egret-metastore/src/lib.rs`; migration append-only (không sửa migration đã áp).
- **Validate `sql_into`** ở cả kernel (identifier) và FE (lỗi inline) để tránh chèn Python không hợp lệ.
- **Reduced-motion** phải gate pulse busy và transition sidebar/collapse.
- **Header sticky + Monaco:** đảm bảo scroll của Monaco không chọi với trang; chiều cao editor cố định với `scrollBeyondLastLine:false` (sẵn có).

## 9. Phân pha đề xuất (cho plan)
- **P1 (thuần FE):** IDE shell + header + `KernelStatusBadge` (+ status stream cho KernelConnection) + port `CellOutputRenderer` + panels + Monaco completions. Ship độc lập được.
- **P2 (lát cắt dọc):** SQL cell + binding — migration 0048, magic `%%egret_sql`, DTO/regen, FE SQL cell + chip.
- **P3 (stretch):** target Daft-local.
