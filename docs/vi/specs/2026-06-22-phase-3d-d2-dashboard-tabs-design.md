# Phase 3d-D2 — Dashboards: tabs (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-22-phase-3d-d2-dashboard-tabs-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-22 · **Tiếp nối:** Phase A/B/C (đã merge PR #25/#26/#27, `903df67`) và D1 (click behaviour, PR #28)

## Mục tiêu

Cho một dashboard có nhiều **tab** có tên, mỗi tab một lưới card riêng. Một thanh tab để
chuyển qua lại; ở edit mode user có thể thêm, đổi tên, sắp xếp lại, xoá, nhân bản tab, và
chuyển một card sang tab khác. Đặc trưng dashboard nổi bật thứ hai của Metabase sau filter —
làm theo cùng mức fidelity với A/B/C/D1.

Phase D chia thành các sub-phase độc lập; đây là **D2 (tabs)**. **D1 (click behaviour)** đã
ra ở PR #28. Public sharing/embedding **ngoài phạm vi Phase D** (egret là tool nội bộ).

## Căn cứ Metabase (clean-room)

Đọc source (`data-portal/metabase`):

- **Mô hình dữ liệu**: bảng `dashboard_tab` `(id, dashboard_id, name, position, entity_id,
  created_at, updated_at)` (`resources/migrations/001_update_migrations.yaml`, v47.00-006),
  cùng cột FK **nullable** `dashboard_tab_id` `ON DELETE CASCADE` trên dòng dashcard
  (v47.00-007). Dashboard **không có tab nào** giữ card ở `dashboard_tab_id = null` và không
  hiện thanh tab; không được trộn tab với card null-tab.
- **Lưu**: một PUT toàn-dashboard `PUT /api/dashboard/:id` mang `{tabs, dashcards,
  parameters}` cùng lúc (`dashboards_rest/api.clj`); không có endpoint riêng cho tab. Tab mới
  gửi kèm id tạm và response trả về mapping temp→real id (`do-update-tabs!`).
- **Ngữ nghĩa**: mỗi card thuộc đúng một tab; mỗi tab là một lưới độc lập (row/col card tính
  trong phạm vi tab); **parameter ở mức dashboard**, dùng chung cho mọi tab, và giá trị
  filter được giữ khi chuyển tab.
- **UX**: thanh tab hiện ở view mode chỉ khi có **≥2 tab**, và luôn hiện ở edit mode. Thao tác
  edit: thêm (`+`), đổi tên (double-click), sắp xếp (kéo thả), xoá (có confirm; chỉ bật khi
  ≥2 tab), nhân bản tab, và chuyển card sang tab. Tab đang chọn phản ánh trên URL. Dữ liệu
  card **tải lazy theo tab đang xem** — chỉ card của tab đang chọn được fetch, quay lại tab
  cũ dùng lại cache.

### egret hiện tại (D2 mở rộng cái gì)

- Bảng `dashboards` (+ `parameters jsonb` từ migration 0016) và bảng `dashboard_cards`
  **đã chuẩn hoá** (`grid_col/grid_row/size_x/size_y`, `options jsonb`, `ordinal`).
- Lưu toàn-layout: `DashboardRepo::replace_cards` xoá toàn bộ card của dashboard rồi chèn lại
  bộ card được gửi trong một transaction (`crates/egret-bi/src/dashboard.rs`); body PUT là
  `{name, description, parameters, cards}`.
- `DashboardView` giữ `draftCards` / `draftParameters` ở edit mode và lưu toàn layout; Phase
  B/C đã có view-state sync URL (`?refresh`, `?fullscreen`, `?slug=`).

## Phạm vi (PR này)

**Có:** bảng `dashboard_tabs` + cột `tab_id` nullable trên card; tab trong GET response và
PUT toàn-lưu; thanh tab (view + edit); thêm / đổi tên / sắp xếp / xoá (kèm reassign) / nhân
bản tab; chuyển card sang tab; render lazy theo tab; tab đang chọn sync URL.

**Defer (nêu rõ):** `entity_id`/timestamps của bảng tab kiểu Metabase (egret không có hệ
serialization/export và `dashboard_cards` cũng không có timestamps); URL tab kiểu id+slug
(D2 định danh tab đang chọn bằng **position**); xoá dashboard về 0 tab (xoá chỉ cho phép khi
≥2 tab); parameter theo tab (parameter ở mức dashboard).

## Mô hình dữ liệu

### Migration `0017_dashboard_tabs.sql`

```sql
CREATE TABLE dashboard_tabs (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id uuid NOT NULL REFERENCES dashboards(id) ON DELETE CASCADE,
    name         text NOT NULL,
    position     int  NOT NULL DEFAULT 0
);
CREATE INDEX dashboard_tabs_dashboard_idx ON dashboard_tabs (dashboard_id);

ALTER TABLE dashboard_cards
    ADD COLUMN tab_id uuid REFERENCES dashboard_tabs(id) ON DELETE CASCADE;
```

`tab_id` **nullable** — "single view ngầm định" kiểu Metabase. Dashboard **0 tab** giữ card ở
`tab_id = null` và không hiện bar, nên **mọi dashboard cũ vẫn chạy, không cần migrate dữ
liệu**. Bar hiện khi dashboard có ≥1 tab.

> Cột bỏ so với Metabase: `entity_id` (egret không có hệ serialization) và
> `created_at/updated_at` (`dashboard_cards` của egret cũng không có — giữ nhất quán).

### Định danh tab khi lưu — delete-all + định danh theo position

Tab theo đúng pattern card sẵn có của egret: lưu-toàn-bộ **xoá hết tab của dashboard rồi chèn
lại** bộ tab gửi lên theo thứ tự. Cách này sinh uuid tab mới mỗi lần lưu (khác diff giữ-id của
Metabase), nên frontend không bao giờ dựa vào id tab cố định — nó **định danh tab đang chọn
bằng position**, cả trong state lẫn URL (`?tab=<position>`). Đây là điểm khác biệt cố ý duy
nhất so với định danh-theo-id của Metabase; hành vi nhìn thấy (deep-link tới tab, chuyển tab,
edit + lưu) giống hệt, và giữ backend đơn giản như `replace_cards`.

## Backend

### Repo (`crates/egret-bi/src/dashboard.rs`)

- `Dashboard` GET thêm `tabs: Vec<DashboardTab>` với `DashboardTab { id, name, position }`
  (sắp theo `position`). Mỗi `DashboardCard` thêm `tab_id: Option<Uuid>`.
- Kiểu input mới: `NewTab { name: String, position: i32 }`; `NewCard` thêm
  `tab_index: Option<usize>` (chỉ số 0-based vào `tabs` gửi lên, hoặc `None` cho dashboard
  không tab).
- `replace_layout(dashboard_id, tabs: &[NewTab], cards: &[NewCard])` mới (thay `replace_cards`)
  chạy trong **một transaction**:
  1. `DELETE FROM dashboard_cards WHERE dashboard_id = $1` (card trước — FK tới tab);
  2. `DELETE FROM dashboard_tabs WHERE dashboard_id = $1`;
  3. chèn từng tab theo thứ tự, gom uuid mới vào `Vec<Uuid>` đánh theo position;
  4. chèn từng card với `tab_id = tab_index.map(|i| tab_ids[i])` (kiểm tra trong khoảng;
     ngoài khoảng, hoặc `tab_index` có giá trị mà `tabs` rỗng → `BiError::Invalid`).
- `cards()` thêm `tab_id` vào `SELECT`; thêm query `tabs(dashboard_id)` trả về các dòng tab.

### API (`dashboard_router`)

- DTO GET thêm `tabs: [{id, name, position}]`; mỗi card DTO thêm `tab_id`.
- Body PUT thành `{name, description, parameters, tabs: [{name, position}],
  cards: [{…, tab_index}]}`. Handler chạy `update` (metadata + parameters) và `replace_layout`
  (tab + card) cùng lúc, đúng như hôm nay chạy `update` + `replace_cards`.
- Validate: `tabs` không rỗng thì mọi card phải có `tab_index`; `tabs` rỗng thì mọi
  `tab_index` của card phải `None`. Sai → 400 (kiểu "không trộn tab với card null-tab").

## Frontend

### Type & API (`frontend/src/api/dashboards.ts`)

- `DashboardTab { id: string; name: string; position: number }`.
- `DashboardDetail` thêm `tabs: DashboardTab[]`; `DashboardCard` thêm `tab_id: string | null`.
- `CardInput` thêm `tab_index: number | null`; body PUT thêm `tabs: { name; position }[]`.

### Tab đang chọn + URL (`?tab=<position>`)

- Một hook nhỏ (giống `useDashboardViewControls`) đọc/ghi `?tab=<position>` và cho ra
  `activeTabPos` + `setActiveTabPos`. Mặc định `0`; ngoài khoảng kẹp về `0`.
- **Mọi hook mới khai báo phía trên** early return `if (isLoading || !dash) return …`
  (bài học Rules of Hooks từng làm `DashboardView` crash).

### Thanh tab (`DashboardTabBar`)

- Hiện các tab theo position. **Hiện** ở view mode chỉ khi `tabs.length ≥ 2`; **luôn** hiện ở
  edit mode (để thêm tab đầu tiên). Dashboard không tab render y như hôm nay (không bar).
- View: bấm tab đặt `activeTabPos`. Edit thêm các thao tác bên dưới.

### Lọc card theo tab + render lazy

- **View mode:** `activeTab = tabs[activeTabPos]`;
  `visibleCards = tabs.length === 0 ? cards : cards.filter(c => c.tab_id === activeTab.id)`.
- **Edit mode:** `activeTab = draftTabs[activeTabPos]`; lọc draft card theo client tab key
  (`card.tabKey === activeTab.key`); draft không tab hiện tất cả card.
- Chỉ `visibleCards` được mount, nên chỉ query card của tab đang chọn chạy. Chuyển tab unmount
  card tab cũ và mount card tab mới; React Query cache nên quay lại tab cũ là tức thì.

### Draft state & thao tác edit

- Thêm `draftTabs: DraftTab[]` cạnh `draftCards`/`draftParameters`. `DraftTab` có client key
  (`new-tab-<n>`), `name`, `position`; draft card tham chiếu tab qua key đó (`card.tabKey`).
  `startEdit` dựng `draftTabs` từ `tabs` của server (mỗi tab một client key mới) và map
  `tab_id` server của mỗi draft card sang key tab tương ứng (dashboard không tab → `draftTabs`
  rỗng và `card.tabKey = null`). Khi lưu, tab serialize theo thứ tự position và `tab_index`
  mỗi card là index của key tab (hoặc `null` khi không có tab).
- **Thêm** (`+`): trên dashboard không tab mà có card, lần thêm đầu tạo **Tab 1 chứa card
  hiện có + Tab 2 rỗng** và kích hoạt Tab 2 (hành vi tab-đầu của Metabase); các lần sau thêm
  một tab rỗng và kích hoạt nó.
- **Đổi tên**: double-click nhãn tab → input inline (Enter/blur commit vào draft).
- **Sắp xếp**: kéo tab để đổi `position` trong draft.
- **Xoá**: control mỗi tab, chỉ bật khi `draftTabs.length ≥ 2`. Nếu tab có card, dialog confirm
  cho chọn **Reassign** (chuyển card sang tab khác) hoặc **Delete cards** (xoá theo tab). Sau
  xoá, `activeTabPos` kẹp lại trong khoảng.
- **Nhân bản tab**: deep-copy card của tab (client id mới, giữ nguyên grid) sang một tab mới
  chèn sau tab nguồn, và kích hoạt nó.
- **Chuyển card sang tab**: thao tác edit theo card (chọn tab khác) đổi tab key của card và
  tính lại ô grid trên lưới 0-based của tab đích (đặt dưới max row hiện tại, như `addCard`).

### Grid

Mỗi tab là một lưới 24-cột, 0-based riêng. Component grid không đổi — chỉ được cấp
`visibleCards` (card của tab đang chọn) thay vì toàn bộ card.

## Luồng dữ liệu

```
GET dashboard → { …, tabs:[{id,name,position}], cards:[{…, tab_id}] }
  view: activeTabPos (?tab=) → activeTab = tabs[pos] → mount card có tab_id===activeTab.id
  edit: draftTabs + draftCards(client tab key) → thao tác thanh tab sửa draft
  save: PUT { …, tabs:[{name,position}], cards:[{…, tab_index}] }
        → replace_layout: xoá card+tab; chèn tab (gom id theo index);
                          chèn card với tab_id = tab_ids[tab_index]
```

## Xử lý lỗi

- PUT `tabs` không rỗng nhưng card thiếu `tab_index` (hoặc `tabs` rỗng mà card có `tab_index`)
  → 400 (không trộn tab với card null-tab).
- `tab_index` ngoài khoảng → 400.
- Xoá bị tắt khi còn 1 tab (không xoá về 0 qua UI).
- Chuyển tới tab có card lỗi: mỗi card hiện trạng thái lỗi riêng (hành vi per-card không đổi);
  thanh tab không bao giờ lỗi.
- Dashboard không tab (không `tabs`) không đổi so với hôm nay — A/B/C/D1 nguyên vẹn.

## Kiểm thử

- **Vitest:** thanh tab render/sắp theo position, ẩn ở view khi <2 tab nhưng hiện ở edit; bấm
  tab đổi `activeTabPos` và sync `?tab=`; chỉ card tab đang chọn được mount (lazy); mỗi thao
  tác draft (thêm gồm split tab-đầu, đổi tên, sắp xếp, xoá kèm reassign vs delete-cards, nhân
  bản, chuyển card) cho `draftTabs`/`draftCards` đúng; payload lưu serialize `tabs` + `tab_index`
  per-card đúng; `?tab=` ngoài khoảng kẹp lại.
- **Backend (test repo `egret-bi`):** `replace_layout` chèn tab theo thứ tự, resolve `tab_id`
  mỗi card theo `tab_index`, round-trip qua `tabs()`/`cards()`, cascade khi xoá tab và xoá
  dashboard, và từ chối input trộn/ngoài khoảng.
- **Live (thủ công, Playwright MCP — egret không có suite Playwright tự động):** thêm tab vào
  dashboard, chuyển card giữa các tab, đổi tên/sắp xếp/xoá (reassign), nhân bản; chuyển tab
  (chỉ card tab đang xem tải); deep-link `?tab=`; xác nhận dashboard không tab không bị ảnh
  hưởng. **0 lỗi console.**
- **Full sweep** trước mỗi commit: từ `frontend/`, `npm run typecheck && npm run lint &&
  npm run build && npx vitest run`; cho thay đổi backend, `cargo test -p egret-bi`; **không**
  cần manual Trino/StarRocks (không đổi engine/catalog).

## Ngoài phạm vi (sau / phase khác)

Public sharing/embedding; parameter theo tab; URL tab id+slug; `entity_id` và serialization
của tab kiểu Metabase; xoá dashboard về 0 tab.

## Quy ước

Metabase là chuẩn · clean-room (không AGPL) · TDD · full sweep trước mỗi commit · identity git
đã cấu hình (không `--author`), footer `Co-Authored-By: Claude Opus 4.8 (1M context)
<noreply@anthropic.com>` · chỉ stage path tường minh · squash-merge · branch
`feat/dashboards-phase-d2` (sau khi D1 merge).
