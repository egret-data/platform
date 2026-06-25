# SQL Lab — Run Selection + Phím tắt (Thiết kế)

**Trạng thái:** Đã duyệt · **Ngày:** 2026-06-25 · **Phạm vi:** chỉ frontend, không đổi backend.
**Sub-project 1/4** trong nỗ lực SQL Lab parity với Superset (3 cái còn lại: table preview + cây
search; results polish; extras — mỗi cái spec/plan riêng).

> Bản chuẩn: `docs/en/specs/2026-06-25-sqllab-run-selection-design.md`.

## Vấn đề
egret SQL Lab chỉ chạy toàn bộ editor qua nút Run, không phím tắt. Superset có **Run / Run
selection** (chạy SQL bôi đen) + **`Ctrl/Cmd+Enter`** — ergonomic dùng nhiều nhất.

## Mục tiêu
- Có selection → chạy **phần bôi đen**; không → chạy toàn bộ. Nút Run đổi label "Run selection"/"Run".
- **`Cmd/Ctrl+Enter`** chạy (theo selection, giống nút). No-op khi đang chạy.

## Thiết kế (chi tiết code xem bản EN)
- **A. Helper thuần `pickRunSql(full, selected)`** (`components/sqllab/pickRunSql.ts`): trả selected
  nếu có chữ (trim), không thì full. Unit-test được (không cần Monaco).
- **B. `SqlEditor`** thêm props `onRun?(sql)` + `onSelectionChange?(selectedText)`. Trong `onMount`
  bắt `editor`: đăng ký keybinding `CtrlCmd|Enter` → `onRun(pickRunSql(full, selected))`; lắng
  `onDidChangeCursorSelection` → `onSelectionChange(selectedText)`. Dùng ref giữ callback mới nhất
  (closure-safe như `wordsRef`); gom disposables thành mảng.
- **C. `SqlTab`** thêm state `selectedSql`; `run(sqlOverride?)` (mặc định `tab.sql`, guard `if(running)
  return`); nút Run: label "Run selection"/"Run" + `title="⌘/Ctrl+↵"`, onClick chạy selection-or-full;
  truyền `onRun={(sql)=>run(sql)}` + `onSelectionChange={setSelectedSql}` xuống editor.

## Kiểm thử
- Unit `pickRunSql`: selected có chữ → selected; whitespace/null/"" → full.
- SqlTab selection path: mock `@/components/sqllab/SqlEditor` bằng stub có nút gọi
  `onSelectionChange("SELECT 1")` + `onRun("SELECT 1")`; assert nút thành "Run selection" và
  `submit.mutateAsync` nhận `sql:"SELECT 1"`; không selection → "Run" + `tab.sql`. Test cũ giữ xanh
  (props mới optional).
- Live (Playwright, 0 console error): bôi đen 1 câu → "Run selection" chạy đúng phần đó; bỏ chọn →
  "Run" chạy toàn bộ; `Cmd/Ctrl+Enter` chạy; inert khi đang chạy.

## Files
- **Mới:** `sqllab/pickRunSql.ts` (+ test). **Sửa:** `sqllab/SqlEditor.tsx`, `sqllab/SqlTab.tsx` (+ test).
  **Backend:** không.

## Rủi ro
- Keybinding/listener đăng ký 1 lần lúc mount → phải đọc callback mới nhất qua ref (tránh stale).
  `CtrlCmd|Enter` chỉ override Enter khi có Cmd/Ctrl; Enter thường vẫn xuống dòng.
