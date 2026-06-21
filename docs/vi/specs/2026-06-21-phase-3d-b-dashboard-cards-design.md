# Phase 3d-B — Dashboards: đa dạng card + hoàn thiện (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-21-phase-3d-b-dashboard-cards-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-21 · **Tiếp nối:** Phase A (lưới card saved-chart, đã merge PR #25, `4a71a1d`)

## Mục tiêu

Bổ sung lát parity Metabase thứ hai cho dashboard egret: **text / heading / link
card**, override **tiêu đề từng card**, hành động **duplicate / replace** card, và
view control **auto-refresh + fullscreen** — bám hành vi Metabase, đúng mức trung
thực của Phase A.

## Vì sao / căn cứ Metabase

Đã đọc source Metabase local (`data-portal/metabase`). Các phát hiện neo thiết kế:

- **Text card** — dashcard ảo `display:"text"`; nội dung ở `visualization_settings.text`;
  render bằng **react-markdown** + remark/rehype GFM (`Text/Text.tsx`).
- **Heading card** — `display:"heading"`, 1 dòng, setting `text`,
  `"dashcard.background": false` (`Heading/Heading.tsx`).
- **Link card** — `display:"link"`, `visualization_settings.link.{url|entity}`
  (`LinkViz/LinkViz.tsx`). egret chỉ làm **URL ngoài** (bỏ entity nội bộ).
- **Tiêu đề từng card** — `visualization_settings["card.title"]`; mặc định = tên
  question; `""` ẩn header (`lib/settings/visualization.ts`).
- **Auto-refresh / fullscreen** — *không* lưu trên dashboard; state context **sync
  URL** (`?refresh=<giây>`, `?fullscreen=true`) qua `useDashboardLocationSync`;
  refresh tính bằng giây: off / 60 / 300 / 600 / 900 / 1800 / 3600 (`RefreshWidget`).
  Refresh dispatch lại query từng card.
- **Duplicate** — clone dashcard với id tạm mới, xếp xuống dưới (`duplicateCard`).
  **Replace** — đổi card trên dashcard hiện có, giữ vị trí lưới, và **xóa
  visualization_settings** (`replaceCard`).

Clean-room: chỉ sao hành vi, không dùng mã AGPL. egret giữ schema riêng (key
`options` jsonb, không phải tên `visualization_settings` của Metabase).

Trim cố ý (đã chốt với user): **không night mode** (egret đã có toggle theme
sáng/tối toàn cục), **không nội suy `{{biến}}`** trong text (phụ thuộc dashboard
parameters → Phase C), **không link tới entity nội bộ**.

## Data model — không migration

`dashboard_cards` của Phase A đã có `card_type text` + `options jsonb`, và backend
đã nhận card `chart_id = NULL` (guard `require_cards_in_ws` chỉ kiểm card *có*
`chart_id`). Nên Phase B **không** đổi backend/SQL. `card_type` và shape `options`
theo loại:

| `card_type` | `chart_id` | `options` |
|---|---|---|
| `chart` | id chart | `{ title?: string }` — override header; `""` ẩn |
| `text` | `null` | `{ text: string }` — markdown |
| `heading` | `null` | `{ text: string }` — 1 dòng |
| `link` | `null` | `{ url: string, label?: string }` |

Type `DashboardCard`/`CardInput` ở frontend đã mang `card_type` và
`options: Record<string, unknown>`; ta thêm accessor có kiểu, không đổi schema.

## Kiến trúc (thuần frontend)

### Dispatcher render card

Hiện `DashboardView` render mọi card qua `CardTile` (chỉ chart). Tách
**`DashboardCardView`** switch theo `card_type`:

- `chart` → đường chart cũ (`useChartData` + `ChartRenderer`), header =
  `options.title ?? chart.name`, ẩn header khi `options.title === ""`.
- `text` → `<TextCard text={options.text} />` — render markdown.
- `heading` → `<HeadingCard text={options.text} />` — 1 dòng, không viền/nền.
- `link` → `<LinkCard url label />` — ô bấm được, mở URL tab mới.

Mỗi loại là một component nhỏ, test độc lập, đặt dưới
`frontend/src/components/chart/dashboard/`. Card chart vẫn tự gọi `useChartData`
(giữ hooks one-per-card — `DashboardCardView` chính là component per-card, mount 1
lần mỗi grid child, nên nhánh chart gọi hook không điều kiện trong nó).

### Markdown

Thêm `react-markdown` + `remark-gfm`. react-markdown **không** render raw HTML mặc
định (không có `rehype-raw`) nên markdown người dùng không chèn script được — an
toàn không cần sanitize thêm. GFM cho bảng / list / autolink / strikethrough.

### Sửa card trong edit mode

Mỗi card sửa tại chỗ khi đang edit:

- **Toolbar Add** thêm **+ Text**, **+ Heading**, **+ Link** cạnh list saved-chart.
  Mỗi nút thêm card loại đó với `options` mặc định (text/url rỗng) xuống dưới
  (cùng định vị `addCard` như Phase A).
- **Editor inline** (chỉ khi edit):
  - text → `<textarea>` bind `options.text`
  - heading → `<input>` bind `options.text`
  - link → input `url` + `label`
  - chart → input "Title" bind `options.title` ở header card
  Sửa làm thay đổi `options` của draft card (cùng state draft-cards của Phase A);
  `options` lưu qua whole-layout PUT sẵn có (không endpoint mới).
- **Hành động từng card** (edit mode, cạnh nút ✕ remove sẵn có):
  - **Duplicate** (⧉) — clone với id tạm mới, xếp xuống dưới (dùng lại định vị
    maxY), copy `options`.
  - **Replace** (chỉ chart card) — mở picker saved-chart; đặt `chart_id` mới,
    **reset `options` về `{}`** (Metabase xóa viz settings khi replace), giữ vị
    trí + size.

### View control (view mode, header) — sync URL

Hook nhỏ **`useDashboardViewControls`** đọc/ghi hai URL param qua react-router
`useSearchParams`:

- `?refresh=<giây>` → `refreshSeconds: number | null` (null = off). Options:
  Off / 1 / 5 / 10 / 15 / 30 / 60 phút (lưu bằng giây, khớp Metabase). Một
  `setInterval(refreshSeconds*1000)` tăng `refreshNonce`; nonce truyền vào
  `useChartData` (arg optional mới) và thêm vào effect deps để mỗi card chạy lại
  build→submit→poll mỗi tick. Reset khi param đổi hoặc unmount.
- `?fullscreen=true` → `isFullscreen: boolean`. Fullscreen là chế độ CSS full
  viewport (container dashboard thành `fixed inset-0 z-50 bg-background`, phủ chrome
  app). Toggle cũng best-effort gọi `requestFullscreen()` / `exitFullscreen()` lúc
  bấm (gesture thật); khi reload với `?fullscreen=true` thì khôi phục chế độ CSS mà
  không gọi native (browser chặn fullscreen không có gesture).

Header (view mode) hiện: dropdown **refresh** + toggle **fullscreen** (cạnh nút
Edit). Cả hai ẩn trong edit mode.

`useChartData` đổi: thêm arg optional `nonce?` đưa vào `specKey`/effect deps để
nonce đổi thì refetch. Không truyền nonce → hành vi cũ không đổi (caller/test Phase
A vẫn chạy).

## Luồng dữ liệu

```
DashboardView (view)
  ├─ useDashboardViewControls()  ⇄  URL (?refresh, ?fullscreen)
  │     └─ refreshNonce (tick)
  └─ mỗi card → DashboardCardView(card, refreshNonce)
        ├─ chart → useChartData(ws, chart, refreshNonce) → ChartRenderer
        ├─ text/heading → markdown / dòng từ options
        └─ link → ô anchor

DashboardView (edit)
  └─ draftCards (Phase A) + editor option inline,
     + addText/addHeading/addLink, duplicateCard, replaceCard
     → Save = whole-layout PUT cũ (options đi theo từng CardInput)
```

## Xử lý lỗi

- Link card URL rỗng → ô disabled/placeholder (không anchor hỏng). Link mở
  `target="_blank" rel="noreferrer"`.
- Text/heading rỗng → render trống (hoặc placeholder mờ khi edit), không crash.
- Replace không hiện trên card không phải chart.
- `?refresh=` sai (không phải số) → coi như off.
- Hành vi card chart (chart bị xóa, query lỗi) giữ như Phase A.

## Kiểm thử

- **Vitest:** `TextCard` (render markdown — heading/list/link), `HeadingCard` (1
  dòng), `LinkCard` (href/target, URL rỗng disabled); dispatcher `DashboardCardView`
  route từng `card_type`; override + ẩn title (`""`); add text/heading/link;
  duplicate (đếm + id mới); replace (đổi chart_id, reset options, giữ vị trí);
  `useDashboardViewControls` (parse param, nonce refresh tick với fake timer);
  `useChartData` refetch khi `nonce` đổi.
- **Live (Playwright):** thêm text + heading + link + 1 chart; đặt title 1 card và
  ẩn title card khác; duplicate + replace 1 chart card; bật auto-refresh (thấy
  re-fetch) + toggle fullscreen; Save → reload (kèm URL param) → tất cả
  persist/khôi phục; **0 lỗi console**.
- **Full sweep trước mỗi commit:** từ `frontend/`,
  `npm run typecheck && npm run lint && npm run build && npx vitest run`.

## Ngoài phạm vi (phase sau)

Filter/parameter dashboard + nội suy `{{biến}}` (Phase C); click behaviour, tabs,
public share (Phase D); night mode và link entity nội bộ (đã cắt).

## Quy ước

Metabase là chuẩn · clean-room (không AGPL) · TDD · full sweep trước mỗi commit ·
identity git đã cấu hình (không `--author`), footer `Co-Authored-By: Claude Opus
4.8 (1M context) <noreply@anthropic.com>` · chỉ stage path tường minh ·
squash-merge PR.
