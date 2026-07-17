# Trang Engine Detail — Thiết kế lại IA (Trino + StarRocks)

**Ngày:** 2026-07-17 · **Trạng thái:** Thiết kế — chờ duyệt · **Nhánh:** `feat/engine-detail-ia-redesign` (off `feat/starrocks-deep-config` / PR #266 — phụ thuộc các section config StarRocks của nó)
**Skill:** ui-ux-pro-max (Data-Dense Dashboard). **Phạm vi đã chọn:** Full IA rework, chỉ FE. Giữ OKLCH/Metabase tokens + shadcn — polish, không đổi style.

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-17-engine-detail-ia-redesign-design.md`.

## Vấn đề (kết quả review)

Trang `EngineDetail.tsx` (giờ dùng chung Trino + StarRocks), soi theo luật Data-Dense Dashboard:

1. **Overview mỏng** — chỉ Status card (bảng pod, hay rỗng) + Danger zone; engine đang provisioning gần như trắng; phải mở tab khác mới biết gì. Thiếu chỉ số tổng quan.
2. **Header sơ sài** — icon/tên/namespace/2 badge; thiếu endpoint `host:port`, warehouse, image, chế độ auth/delegation.
3. **Không có KPI strip** — egret đã chuẩn hoá `SummaryStrip` cho trang CP; engine detail chưa dùng.
4. **State rỗng/loading yếu** — "No pods yet." không lý do/bước tiếp; không skeleton.
5. **Vị trí action lệch** — Restart trong Status card; Save trong từng config tab; không neo cấp-trang.
6. **6 tab phẳng**; Configuration + Security đều là "settings", vốn (cả 2 engine) đã DÙNG CHUNG một form instance + một Save re-provision — nhưng bị tách; nav config StarRocks thiếu badge "khác-default" như Trino.
7. **Monitor chỉ cho Trino** — `EngineMonitor` là query-monitor Trino (cluster, query đang chạy, stage, kill) gọi API native Trino. StarRocks render cùng view Trino → rỗng/hỏng.

## Mục tiêu

Sắp xếp lại IA để cả 2 engine đọc như một ops dashboard mạch lạc:
- **Overview** = dashboard tổng quan (KPI strip + header giàu thông tin + state thật).
- **Settings** = gộp Configuration + Security thành 1 tab (1 left-nav + 1 Save), vì chúng đã dùng chung form instance và đều re-provision.
- **Monitor** = theo engine: giữ query-monitor Trino; cho StarRocks một view **sức khoẻ pod/cluster** dựng từ status k8s egret đã poll (không cần backend mới). Monitor cấp-query cho StarRocks = tính năng tương lai (cần StarRocks query/session API — ngoài phạm vi).
- Action cấp-trang, state, mật độ, và parity Trino↔StarRocks nhất quán.

**Tabs:** `Overview · Catalogs · Workspaces · Settings · Monitor` (5, giảm từ 6).

## Ngoài phạm vi

- **Không backend mới** — Monitor StarRocks dùng status poll sẵn có (pods/phase/replicas/HPA); không query API.
- **Không đổi style** — giữ OKLCH/Metabase tokens, shadcn, ngôn ngữ card hiện tại. Đây là IA + mật độ thông tin + state, không phải style mới.
- **Không đổi logic tab Catalogs/Workspaces** — chỉ polish density/state/hover.
- **Không đổi logic form config/security** — gộp chỉ là trình bày (cùng hook, cùng Save).

## Bối cảnh (đã kiểm chứng)

`EngineDetail.tsx` `EngineTabs`: 6 tab. Config + Security đã gate giống nhau cho cả 2 engine, đều dùng 1 form hook (`useEngineConfigForm` Trino / `useStarRocksConfigForm` StarRocks) + 1 `SaveBar`. Overview = `StatusCard` (bảng pod + Restart) + `DangerCard`. `SummaryStrip` = KPI row chuẩn CP. `EngineMonitor.tsx` = query-monitor Trino, render ungated ở dòng 263 cho mọi engine. Engine object có: type/namespace/status{phase,pods,message}/spec/warehouseId + host/port helper; catalogs+workspaces là query riêng đã dùng.

## Kiến trúc

**A. Header giàu thông tin:** giữ icon/tên/namespace/badge; thêm **hàng metadata** — image · endpoint `host:port` (mono, tabular-nums) · tên warehouse · chế độ auth/delegation. Đưa **action cấp-trang** vào đây: `Restart` (theo kind, từ #265) + menu `⋯` (Delete→confirm). Action cố định qua các tab.

**B. Overview = ops dashboard:** `EngineSummaryStrip` (tái dùng `SummaryStrip`) — item theo engine: Phase · Pods `ready/total` · replicas (`FE·CN` StarRocks / `Coord·Worker` Trino) · #Catalogs · #Workspaces · Auth. `StatusCard` giữ (row-hover + Logs/Exec + **state thật**: provisioning/rỗng → skeleton + "Waiting for the operator…"; lỗi → status.message). `DangerCard` de-emphasize.

**C. Settings (gộp Config + Security):** 1 tab = left-nav config hiện tại + **Security là (các) nav item cuối**, 1 content pane, 1 `SaveBar`. Trino: Image·Server·Coordinator·Worker·Workload·Pod·Autoscaling·Security. StarRocks: Image·Compute·Resources·FE config·CN config·Autoscaling·Pod·Storage·Security/Delegation. Tái dùng hook + Save nguyên vẹn; thêm **badge khác-default cho nav StarRocks**. Giữ deep-link `?section=`.

**D. Monitor theo engine:** Trino giữ nguyên; StarRocks = view **health** từ status poll + spec (pod cards: phase/ready/restarts/age, tóm tắt replica FE/CN, HPA target nếu có, link Logs/Exec) + note mờ "Query-level monitoring for StarRocks coming soon". Không backend mới. `EngineDetail` rẽ Monitor theo `engine.type`.

**E. Density + state pass (cả 2 engine, mọi tab):** padding chặt hơn (density 8), tabular-nums cho cột số/pod, row-hover (150–300ms) + cursor-pointer, nhịp 8px, skeleton cho tab async, tôn trọng reduced-motion, contrast ≥4.5:1.

## Luồng dữ liệu

Không data mới. Header/SummaryStrip đọc `engine`+`status` đã fetch + đếm catalogs/workspaces (query nhỏ sẵn có). Settings tái dùng hook config/security + Save (PUT re-provision, từ #265). Monitor StarRocks đọc `engine.status`.

## Kiểm thử

- **Unit (Vitest):** EngineDetail render 5 tab; SummaryStrip theo engine (FE/CN vs Coord/Worker); Settings render left-nav gộp + Security cuối + 1 SaveBar; Monitor StarRocks hiện health view (không phải query-monitor Trino); copy provisioning; Restart/Delete ở header; Trino query-monitor vẫn chạy.
- **Không test backend** (FE-only).
- **Live (browser):** 1 Trino + 1 StarRocks — header metadata, KPI strip, Settings gộp (1 Save), Monitor StarRocks health, Trino query-monitor còn chạy. Screenshot 1440/1024/768.

## Rủi ro

1. Gộp Config+Security 1 left-nav không được làm hỏng shared-Save/dirty/validity (cả 2 engine) hay deep-link `?section=` — refactor trình bày, giữ nguyên hook + SaveBar.
2. Health view StarRocks phải xử lý `status.pods` rỗng (provisioning) — tái dùng empty-state Overview.
3. **Phụ thuộc #266** — branch off nó; #266 đổi thì rebase.

## Phân rã (cho plan)

1. Header giàu thông tin + Restart/⋯(Delete) cấp-trang. 2. Overview SummaryStrip theo engine. 3. State rỗng/loading + density pass (dùng chung). 4. Gộp Settings (fold Security vào config nav, 1 SaveBar, giữ `?section=`; badge StarRocks). 5. Monitor theo engine (health StarRocks; Trino giữ). 6. Live smoke + screenshot (Trino + StarRocks).
