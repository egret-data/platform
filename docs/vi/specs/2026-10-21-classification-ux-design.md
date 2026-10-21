# Khu vực Classification: sửa các vấn đề khả dụng (Gói 1)

> Bản tham khảo tiếng Việt. Bản tiếng Anh `docs/en/specs/2026-10-21-classification-ux-design.md` là bản chuẩn; hai bản lệch nhau thì theo bản tiếng Anh.

Trạng thái: thiết kế, 2026-10-21. Chưa triển khai. Kế hoạch: [`../../en/plans/2026-10-21-classification-ux.md`](../../en/plans/2026-10-21-classification-ux.md) (kế hoạch không có bản tiếng Việt).
Chương trình: Control Plane & DG, tiếp theo #4b Classification (#702). Các quyết định phạm vi do controller đưa ra trước spec này; tài liệu này triển khai chúng và chỉ nêu những chỗ mà code khiến không làm được (§12).

## 1. Vấn đề

Người dùng thấy Governance > Classification "khá khó dùng". Hai lượt đi thử trên trình duyệt với demo stack (2026-10-21) đã đo vì sao. Mã phát hiện bên dưới là của hai lượt đó: **F1..F8** (lượt 1) và **N1..N26** (lượt 2; lượt 2 sửa lại F4, F5 và F8).

Việc thử ở lượt 2 là: *che cột `email` của `demo_warehouse.analytics.customers` với mọi người trừ nhóm `data-team`, bằng một classification mới `uxwalk-pii`*. Kết quả:

- **Không hoàn thành được một cách kiểm chứng** (N1): không engine nào phục vụ các catalog demo, và Explain không báo mask trên Trino.
- **36 lần nhấp** cho việc chính, cộng 15 lần cho một đường vòng do UI gây ra (N2, N3): tổng 51. Khoảng 9 ô phải gõ, một ô gõ hai lần.
- **8 bề mặt trong 3 khu vực**, **4 lần phải đoán**:
  1. gắn classification ở đâu (F1);
  2. tên cột mở một side sheet;
  3. viết rule ở đâu (F2, N11);
  4. gỡ classification cấp bảng thế nào (N2).
- **2 nút bị vô hiệu mà không nói lý do** (N6), và **3 thông báo lỗi vô ích**:
  - một lời từ chối 600 ký tự về nội bộ engine (N3);
  - một thông báo `conflict:` thô (N10);
  - một toast nêu tên `perm.rules` (N14).

Phần lớn phát hiện quy về ba nguyên nhân.

1. **UI không nói điều nó biết.**
   - Trang tag không bao giờ nói classification đang làm gì (F2).
   - Clearance chỉ nêu giới hạn của nó sau lần thêm đầu tiên (N7).
   - Preview báo phép tính profile thay vì tác dụng (N5).
   - Mask thứ hai trên một cột được chấp nhận im lặng (N4).
2. **Cách sửa nằm ở chỗ khác.**
   - Classification gắn trong Governance chỉ gỡ được trong Control Plane, cách 8 lần nhấp (N2).
   - Không thể bắt đầu một rule từ chính classification mà rule nói đến (N11).
   - Xung đột cấp bảng chỉ lộ ra ở Preview, sau khi đã gắn (N3).
3. **Một thứ có nhiều tên.** Bảng từ vựng của lượt 2 liệt kê 12 khái niệm, mỗi khái niệm có 2 đến 8 tên (N18). Breadcrumb của trang rule còn ghi "People & groups" khi đi từ Classifications (N13).

Những gì đang tốt và phải giữ (mục "Works well" của lượt 2):

- hộp thoại tạo ở 390 px;
- key tự sinh slug;
- Where used nhóm theo schema, có link từng cột;
- "Confidential from table" cho classification kế thừa;
- side sheet của cột;
- chốt preview;
- danh sách hệ quả trong hộp thoại xoá;
- huy hiệu khoá;
- nói "Nothing is enforcing" thay vì "Allowed";
- không cuộn ngang ở cấp trang.

## 2. Nguyên tắc

| # | Nguyên tắc | Loại bỏ điều gì |
|---|---|---|
| P1 | **Nói điều UI biết.** Nếu server hoặc trang đã biết một sự thật người dùng cần (xung đột, chồng lấn, giới hạn, số engine), hãy hiện nó trước khi người dùng hành động, ngay tại control hành động. | Banner chỉ hiện sau khi ghi; lời từ chối là lần đầu báo một xung đột đã biết. |
| P2 | **Hiện tác dụng, không phải cơ chế.** Mở đầu bằng điều người đọc dữ liệu sẽ thấy ("email bị hash với 12 người, 3 người được miễn trừ"). Cơ chế (profile, số compile, usersync) nằm sau một phần mở rộng. | "Would recompile no profiles · created 0 …" làm câu chính. |
| P3 | **Sửa ngay chỗ người dùng đứng.** Mọi đối tượng hiện một classification đều gỡ được nó; mọi classification đều bắt đầu được một rule; mọi lời từ chối có nêu cách sửa thì đưa cách sửa thành một control. | Đường vòng sang Control Plane; bảo "sửa rule" khi rule còn chưa tồn tại. |
| P4 | **Một từ cho một thứ.** Bảng thuật ngữ (§4) là từ vựng duy nhất của khu vực này; chuỗi nào nêu một khái niệm trong đó thì dùng đúng thuật ngữ đó. | "tag", "egret rule", "Masking rules", "Reaches this tag", "Carries it". |

## 3. Quyết định

Mỗi quyết định nêu code đang làm gì hôm nay (đã kiểm, `file:line` trên `origin/main` `265c81c6`), hành vi mới, thay đổi API nếu có, ghi chú a11y và responsive, và cách kiểm thử. Đường dẫn tính từ gốc repo. `FE` nghĩa là `frontend/src/`.

### C1. Một bảng thuật ngữ, áp cho mọi chuỗi trong khu vực (N18, N13, F6, F7)

**Hôm nay.** Một khái niệm có nhiều tên:

- Nhãn tab là "Mask & filter", "View exposure" và "Hand-written" (`FE/routes/governance/classifications/TagPage.tsx:23-29`).
- Một tiêu đề mục là "Mask and filter rules" (`FE/routes/governance/TagRulesSection.tsx:78`). Một tiêu đề khác là "Masking rules" (`FE/routes/governance/data/ColumnPanel.tsx:234`; tiêu đề cột ở `FE/routes/governance/data/tabs/OverviewTab.tsx:306`).
- "egret rule" xuất hiện ở `TagRulesSection.tsx:40,80`.
- Tab của bảng là "Masks & filters".
- Hộp thoại rule ghi "Selects by: Tag/Path" (`FE/routes/governance/RuleDialog.tsx:416-425`) và đặt nhãn ô là "Tag" (`:83`).
- Header của Data liệt kê "Tags" (`FE/routes/governance/data/ObjectHeader.tsx:164`).
- Where used có cột "Carries it" (`FE/routes/governance/classifications/tabs/WhereUsedTab.tsx:144`).
- Danh sách rule hiện kiểu mask ở dạng thô: `formatRuleKind` in ra `mask · MASK_HASH` (`FE/routes/governance/permFormat.ts:162-169`), trong khi hộp thoại gọi cùng giá trị đó là "Hash" (`RuleDialog.tsx:41-50`).
- Header của trang rule luôn link về "People & groups", bất kể đi vào từ đâu (`FE/routes/governance/people/itemParts.tsx:16-18`).
- Link "Open in Data" trong ứng dụng lại mang biểu tượng link ngoài (`FE/routes/governance/openInData.tsx`, `ExternalLink` trong `OpenInDataLink`).

**Mới.**

1. Module `FE/routes/governance/glossary.ts` xuất các thuật ngữ của §4 dưới dạng hằng, cùng `MASK_TYPE_LABEL` (chuyển từ `RuleDialog.tsx:41-50`).
2. Mọi chuỗi trong khu vực Classification, các bề mặt classification và rule của khu vực Data, hộp thoại rule và trang rule đều dùng các hằng đó.
3. `formatRuleKind` in ra `Mask · Hash` và `Row filter · region = 'us'`.
4. Các tab thành **Where used · Clearance · Rules · Views · Engine policies**. Khoá URL giữ nguyên `where-used|clearance|rules|views|handwritten` (`FE/routes/governance/classifications/tagUrl.ts:14`), nên link và bookmark vẫn chạy.
5. Lối quay lại của trang rule nêu đúng nơi người dùng đến từ. `ItemHeader` nhận thêm `back: {to, label}` tuỳ chọn. Các nơi gọi `RuleDialog` và tab Rules link tới rule kèm `?from=classification:<id>` hoặc `?from=data:<encoded path>`. `RulePage` đọc `from` và hiện "← <tên classification>" hoặc "← <bảng>", mặc định "← People & groups".
6. `OpenInDataLink` dùng `ArrowRight` thay cho `ExternalLink`, vì mọi chỗ dùng đều trong ứng dụng. Các file import: `openInData.tsx`, `people/RulePage.tsx`, `people/GrantPage.tsx`, `people/tabs/MasksFiltersTab.tsx`.

**Ngôn ngữ UI.** Chữ trên UI vẫn là tiếng Anh (FE-COPY-1). Frontend không có lớp i18n: `grep -rln "i18next\|useTranslation" frontend/src` không tìm thấy gì. Cột tiếng Việt của bảng thuật ngữ dành cho tài liệu tiếng Việt (bản này và sau này `egret-data/docs`), không dành cho chuỗi UI. Xem §12, mục 1.

**Kiểm thử.** Với mỗi bề mặt, một vitest kiểm tên mới qua `getByRole` và kiểm tên đã bỏ không còn (FE-COPY-3). Một unit test ghim `formatRuleKind`.

### C2. Trang tag nói classification làm gì: dải Effect (F1, F2, N11)

**Hôm nay.** `TagPage` vẽ header rồi các tab (`TagPage.tsx:129-147`). Không có gì tóm tắt tác dụng.

- Where used khi trống ghi "No table or column carries this classification." mà không có cách gắn (`WhereUsedTab.tsx:134`).
- Mask & filter khi trống ghi "No egret rule selects this tag." mà không có hành động (`TagRulesSection.tsx:40`).
- Không lối vào nào mở `RuleDialog` với classification này. `RuleDialog` nhận `initialSelector`/`initialKind` (`RuleDialog.tsx:163-179`); chỉ tab Masks của Data truyền chúng (`FE/routes/governance/data/tabs/MasksFiltersTab.tsx:239-246`).

**Mới.** Component `EffectStrip` (`FE/routes/governance/classifications/EffectStrip.tsx`) nằm giữa `TagHeader` và các tab. Nó vẽ một `<dl>` gồm ba sự thật, mỗi sự thật kèm một hành động.

| Sự thật | Nguồn | Chữ (ví dụ) | Hành động |
|---|---|---|---|
| Rules | `GET /perm/rules` + `rulesOnTag` (`FE/routes/governance/ruleMatch.ts:62`), chỉ khi có `policy:read` | "Mask · Hash: Hash email" (và "and N more") / "No rule uses this classification yet." | **Add rule** (cần `policy:update`): mở `RuleDialog` với kind `mask`, kiểu mask `MASK` (Redact), **Applies to** = classification này, tên "Mask <tên>". Không có `policy:read`: "Reading rules needs the policy:read permission." |
| Clearance | `tag.required_roles` / `required_groups` (`FE/api/tags.ts:36-42`) | "Only 2 groups may open data with it" / "None: anyone who can read the data can open it." | **Set clearance** (cần `tag:manage`): chọn `?tab=clearance`. |
| Where used | `GET /classifications/{id}/usage` (`tableCount`, `columnCount`, `bi.readable.length`) | "2 tables · 3 columns · 0 BI objects" / "Not on any data yet." | **Attach in Data**: link tới `/governance/data`, kèm câu "Open a table or a column and use Add classification." |

Khi không có rule, không có clearance và không có nơi dùng, dải này mang dòng trạng thái **"Does nothing yet."** Trạng thái trống của tab Rules và Where used nêu cùng hành động (một link; FE-PAGE-3: lối vào là link, hoặc nút mở hộp thoại tại chỗ).

**A11y.** Dải là một `<section aria-label="Effect">`. Mỗi hành động là `Button` hoặc `Link` thật. Thiếu quyền thì nói bằng chữ cạnh sự thật (FE-ACTION-4), không bao giờ âm thầm thiếu control. **Responsive.** Ở 390 px, `<dl>` xếp mỗi sự thật một hàng (`grid-cols-1 sm:grid-cols-3`).

**Kiểm thử.** `classifications/__tests__/EffectStrip.test.tsx` gồm:

- ba trạng thái của mỗi sự thật (có dữ liệu, trống, thiếu quyền);
- Add rule mở hộp thoại với key của tag đã chọn sẵn, và thân rule được gửi đi preview;
- Set clearance chuyển sang `?tab=clearance`.

### C3. Lần dùng đầu: trang Classifications trống giải thích việc cần làm (m)

**Hôm nay.** `EmptyState` với "No classification yet." và nút New (`FE/routes/governance/classifications/ClassificationsIndex.tsx:194`).

**Mới.** Trạng thái trống có ba bước đánh số:

1. **Tạo** một classification (nút New).
2. **Gắn** nó vào bảng hoặc cột trong **Data** (link tới `/governance/data`).
3. **Thêm rule** (mask hoặc lọc dòng) hoặc **đặt clearance** trên trang của classification.

Các bước cũng hiện cho người không có `tag:manage`, chỉ thiếu nút của bước 1; thay vào đó có câu "Creating needs the tag:manage permission."

**Kiểm thử.** Một vitest trong `ClassificationsIndex.test.tsx` kiểm ba bước, đích của link và biến thể theo quyền.

### C4. Chip classification: màu, link, khoá, và gỡ ngay chỗ nhìn thấy (N2, N16, N17)

**Hôm nay.**

- Chip là `Badge variant="secondary"` màu xám, có tên và key ở `title`. Không màu, không link, không khoá:
  - header của đối tượng (`ObjectHeader.tsx:175-181`);
  - panel cột (`ColumnPanel.tsx:266-279`);
  - bảng cột (`OverviewTab.tsx:364-375`).
- `TagDto` đã có `color` (`FE/api/generated/model/tagDto.ts`).
- Không bề mặt Governance nào gỡ được một lần gán. Chỗ gỡ duy nhất là Control Plane > Catalogs > Browse > Tags (`FE/components/access/CatalogTagEditor.tsx:186-193`, qua `useUnassignResourceTag`, `FE/api/catalogTags.ts:71-91`).
- Lệnh gỡ đó gọi `DELETE /api/control-plane/warehouses/{warehouse_id}/resource-tags` với thân JSON `{schema, table, column?, tag_id}`. Handler là `unassign_resource_tag`, chặn bằng `catalog_registry:update` (`crates/egret-bi/src/catalog_tags.rs:711`, `WAREHOUSE_WRITE` ở `:32`). Nó idempotent (200 khi không khớp gì).
- Các dòng Where used chỉ là link (`WhereUsedTab.tsx:148-162`). `TableUsage` có `catalogId` nhưng không có `warehouseId` (`crates/egret-controlplane/src/governance_tag_usage.rs:56-65`).
- Bộ chọn Add tag chỉ hiện tên trần (`FE/routes/governance/data/AddTagDialog.tsx:131-135`).

**Mới.**

1. **`ClassificationChip`** (`FE/routes/governance/ClassificationChip.tsx`, nằm ngoài các khu vực lazy để cả hai cùng import được). Nó hiện:
   - một ô màu theo màu của classification (`style={{ backgroundColor }}` inline với giá trị đã lưu; màu là dữ liệu người dùng, không phải design token, như `TagSwatch` đã làm ở `FE/routes/governance/classifications/tagParts.tsx:25-33`);
   - tên là link tới trang classification (chỉ khi có `tag:read`; không thì là chữ thường);
   - biểu tượng `Lock` kèm chữ sr-only "requires clearance" khi classification có clearance (đọc từ `useTags()` theo id);
   - "from table" với chip kế thừa;
   - nút gỡ (`IconButton`, `aria-label="Remove <tên> from <đối tượng>"`) khi có `onRemove`.

   Nó thay cho ba cách vẽ chip xám.
2. **Remove** có trên chip khi người gọi có `catalog_registry:update` (hoặc là platform admin) và chip là của chính đối tượng (chip kế thừa ghi "from table" và link sang bảng). Nó mở `RemoveClassificationDialog` (`FE/routes/governance/RemoveClassificationDialog.tsx`), một `AlertDialog` không phải gõ (quy ước C11):
   - tiêu đề "Remove <tên> from <đường dẫn>?";
   - nội dung: "Rules that apply to <tên> stop applying here." Có `policy:read` thì liệt kê tên các rule đó (những rule `rulesOnTag` trả về); với một bảng thì thêm "Its columns stop inheriting it.";
   - hai nút **Cancel** và **Remove**.

   Nó gọi `DELETE …/resource-tags` qua `useControlPlaneCatalogUnassignResourceTag` được sinh ra, vô hiệu đúng những query `AddTagDialog` vô hiệu (`AddTagDialog.tsx:80-93`), và toast "Removed <tên> from <đối tượng>." (FE-ACTION-2: vẫn ở lại trang).
3. Các dòng **Where used** có thêm hành động **Remove** cho từng lần gán: "Table" và từng cột. Hành động nằm trong `ACTIONS_CELL` được ghim (`FE/routes/governance/tabParts.tsx:51`). Việc này cần id của warehouse, nên **`TableUsage` có thêm `warehouseId: Uuid`** (backend B5).
4. **Bộ chọn Add classification** vẽ mỗi lựa chọn kèm:
   - ô màu, tên, key (font mono), dòng đầu của mô tả;
   - huy hiệu khoá "Clearance" khi có clearance;
   - "N rules" khi người gọi có `policy:read` và `rulesOnTag` tìm thấy rule.

   `SelectItem` giữ tên làm giá trị chữ (để gõ tìm), phần còn lại ở dòng phụ. Tiêu đề hộp thoại thành **"Add classification"**, và nút trên header và panel cột đổi tên theo (C1).

**A11y.** Link và nút gỡ của chip là hai điểm focus. Nhãn nút gỡ nêu classification và đối tượng. Ô màu `aria-hidden` (tên đã mang nghĩa, FE-A11Y-4).

**Kiểm thử.**

- `__tests__/ClassificationChip.test.tsx`: style ô màu, đích link, chữ khoá, nút gỡ chỉ hiện khi có quyền, chip kế thừa không có nút gỡ.
- `data/__tests__/ObjectPage.test.tsx` và `ColumnPanel.test.tsx`: gỡ gửi `DELETE …/resource-tags` với đúng thân và tải lại.
- `classifications/__tests__/WhereUsedTab.test.tsx`: Remove trên dòng cột gửi `{schema, table, column, tag_id}` tới `warehouseId` của dòng.

### C5. Classification cấp bảng và mask theo classification: cảnh báo trước, giải thích ngắn (N3)

**Hôm nay.** egret từ chối cặp này trên mọi đường ghi bằng một câu 600 ký tự (`crates/egret-policy/src/unreadable_table_mask.rs:257-275`, `UnreadableTableMask::refusal`). Cặp đó là: một classification gắn vào cả bảng, cộng một mask chọn theo classification đó. Engine kiểm kiểu mask theo tag trên mọi cột, nên bảng thành không đọc được.

| Lần ghi | Ở đâu | Trạng thái hôm nay |
|---|---|---|
| Tạo, sửa và **preview** rule (`Mask` + `Selector::Tag`) | `crates/egret-perm-compile/src/routes.rs:1477-1491` → `PermHttpError::BadRequest` | 400 `{"error":"bad request: <câu>"}`, không có `code` (`crates/egret-perm-compile/src/routes/error.rs:23-24`) |
| Gắn cấp bảng (`column == ""`) khi đã có mask chọn classification | `crates/egret-bi/src/catalog_tags.rs:119-136` → `BiError::BadRequest` | 400, không có `code` |
| Đổi tên thành một tên đang bị mask | `crates/egret-bi/src/tags.rs:198-207` | 400, không có `code` |

Lượt đi thử đã gắn trước (được chấp nhận vì lúc đó chưa có mask), rồi gặp lời từ chối ở Preview. `RuleDialog` hiện nguyên văn thông báo (`RuleDialog.tsx:314`). Struct mang `mask_origin`, `mask`, `tag` (tên), `schema` và `table` (`unreadable_table_mask.rs:208-217`). Nó không mang id của tag hay id của warehouse, và `table_level_attachment_of` dừng ở bảng đầu tiên (`LIMIT 1`, `:403-428`).

**Mới, backend (B2).**

1. `UnreadableTableMask` có thêm `tag_id: Uuid`, `warehouse_id: Option<Uuid>` và `more_tables: bool`. `table_level_attachment_of` chọn `t.id, a.warehouse_id` và dùng `LIMIT 2`; dòng thứ hai bật `more_tables`. Nhánh Mask và Rename điền `warehouse_id` từ dòng đó. Nhánh Attachment đã biết `tag_id` và để `warehouse_id` là `None`: lần ghi đang diễn ra *chính là* lần gắn cấp bảng, nên không có gì để gỡ. Bản thân `TagWrite` không đổi, nên 20 chỗ gọi trong test không bị đụng.
2. Phương thức mới `UnreadableTableMask::refusal_body() -> serde_json::Value` trả về:

   ```json
   {
     "error": "<câu hiện có, giữ nguyên>",
     "code": "table_tag_mask_conflict",
     "tag": "PII", "tagId": "…", "warehouseId": "… hoặc null",
     "schema": "analytics", "table": "orders",
     "mask": "uxwalk mask email", "maskOrigin": "rule",
     "moreTables": false
   }
   ```

   `maskOrigin` là `rule` hoặc `policy`.
3. Cả ba đường ghi trả nó với mã 400, qua một biến thể mới trên mỗi kiểu lỗi, trả nguyên thân có mã:
   - `PermHttpError::Coded { status, body }`;
   - `BiError::Coded { status, body }`, ánh xạ sang `CpError::Coded(status, body)`.

   Nơi gọi thứ tư, trình sửa của Engine & policy (`crates/egret-policy/src/admin/mod.rs:1823-1826`), giữ 400 thường: nó nằm ngoài khu vực này, và câu của nó đã nêu tên policy đang sửa.

   Đây là cùng mẫu mà `Overlap`/`RuleOverlap` đang dùng (`crates/egret-perm-compile/src/routes/error.rs:38`, `crates/egret-bi/src/error.rs:27-28,102`, `crates/egret-controlplane/src/error.rs:35-36,150-152`). Câu vẫn nằm trong `error`, nên client cũ vẫn chạy.

**Mới, frontend.**

1. **Hộp thoại rule.** Khi gặp `ApiError` có `code === "table_tag_mask_conflict"`, vùng preview hiện một câu: **"<tag> is on the whole table <schema>.<table>, so masking by <tag> would make that table unreadable."** Kèm hai hành động:
   - **Remove from table**. Cần `catalog_registry:update`; gọi `DELETE …/resource-tags` với `{schema, table, column: "", tag_id}` trên `warehouseId`, rồi chạy lại preview.
   - **Apply by path instead**, chuyển Applies to = Path.

   Khi `moreTables` là true, thêm "Other tables carry it too; each will be shown after this one." Câu của server nằm trong phần mở rộng "Details" (`<details>`). Không có quyền thì Remove được thay bằng "Removing needs the catalog_registry:update permission."
2. **Hộp thoại Add classification, đích là bảng.**
   - Có `policy:read`: khi classification được chọn có rule mask (`rulesOnTag` → `kind.type === "mask"`), hộp thoại hiện xung đột **trước** khi gửi, vô hiệu **Add** (aria-disabled, gắn lý do), và đề nghị **Attach to columns instead**. Hành động này chuyển form sang danh sách checkbox các cột của bảng và gửi một `PUT` cho mỗi cột được tích.
   - Không có `policy:read`, hoặc chưa có mask nào: hộp thoại hiện ghi chú ngắn dưới bộ chọn: "On a whole table, this classification counts for clearance on every column. A rule that masks by classification needs it on columns, not on the table."
   - Danh sách cột lấy từ trang đối tượng, nơi đã có sẵn (`TagTarget` có thêm `columns?: string[]`).
   - Lỗi 400 của server có mã này hiện bằng cùng câu ngắn cạnh bộ chọn.
3. **Đổi tên** (form sửa ở TagHeader) hiện câu ngắn ứng với mã này.

**Kiểm thử.**

- Backend:
  - `crates/egret-perm-compile/tests/routes.rs`: preview một mask theo tag trên một lần gắn cấp bảng trả 400 với `code`, `tagId`, `warehouseId`, `schema`, `table`, `maskOrigin` = `rule`, và `error` vẫn bắt đầu bằng `tag "`.
  - `crates/egret-bi/tests/catalog_tags_routes.rs`: gắn cấp bảng trả cùng mã, `maskOrigin` theo từng bề mặt (mở rộng test có sẵn `a_table_level_assignment_is_refused_when_a_tag_mask_names_the_tag`).
  - Unit test trong `crates/egret-policy/src/unreadable_table_mask.rs`: `refusal_body()["error"] == refusal()`.
- Frontend:
  - `__tests__/RuleDialog.test.tsx`: câu ngắn, Remove from table gửi đúng thân và preview lại, Apply by path.
  - `data/__tests__/AddTagDialog.test.tsx` (tạo mới): cảnh báo trước khi gửi khi có rule mask, đường đi theo cột, và ghi chú khi không có `policy:read`.

### C6. Mask thứ hai trên một cột không bao giờ im lặng (N4)

**Hôm nay.**

- `GET /perm/overlaps` (`routes.rs:134`, `list_overlaps` ở `:1769`) và 409 lúc ghi (`refuse_created_by_compile_via`, `crates/egret-perm-compile/src/overlap.rs:249-314`) phát hiện các cặp mask có thể quyết định một request khác nhau, kể cả tag-và-path và tag-và-tag trên một cột (`crates/egret-policy/src/unit_overlap.rs:402-590`).
- Cả hai so **bản tải đã compile theo từng profile**. Khi không profile gắn engine nào phục vụ catalog, như trên demo stack, không có gì được compile, nên không phát hiện được gì. Đó đúng là trường hợp của lượt đi thử: mask thứ hai trên `email` với miễn trừ khác được chấp nhận không một cảnh báo.
- Miễn trừ được compile thành một item `MASK_NONE` đặt trước item "mọi người" (`crates/egret-perm-compile/src/rules.rs:43-44,764-778`). Với hai mask policy trên một cột, cái nào áp dụng là do thứ tự đánh giá quyết định. Vì thế egret từ chối cặp đó khi nó nhìn thấy được.
- Thân 409 là `{error, code: "rule_overlap", rules: [{id, name}], coordinate, kind}` (`crates/egret-perm/src/actions.rs:509-529`).

**Mới.**

1. **Phát hiện ở mức mô hình trong Preview (B3).** `RuleEffect.alreadyMasked` liệt kê các mask rule khác chạm tới bất kỳ cột nào mà rule này chạm tới. Nó được tính từ các rule và các lần gán classification, độc lập với profile. Mỗi mục là `{ruleId, ruleName, maskType, coordinate, exempt}`, tối đa 20 (đặc tả hàm ở §6, B3).
2. **Hộp thoại rule hiện điều đó trước Create:**
   - "email is already masked by **Hash personal data** (Hash, except group data-stewards)." Tên rule link tới trang của nó.
   - **Kết quả thực tế theo nhóm.** Mỗi principal được nêu trong bất kỳ miễn trừ nào có một dòng, cộng dòng "Everyone else". Các dòng này đến từ một hàm thuần `effectivePerPrincipal(newRule, alreadyMasked)` trong `FE/routes/governance/ruleEffect.ts`:

     | Principal được miễn trừ khỏi | Dòng |
     |---|---|
     | mọi mask trên cột | "sees the real value" |
     | không mask nào | "masked (<kiểu>)". Nếu các kiểu khác nhau: "masked; which mask applies is decided by engine order" |
     | một số nhưng không phải tất cả | "**undecided**: the engine applies whichever mask it evaluates first" |

   - Một checkbox bắt buộc, **"Create anyway: I understand email will have two masks"**. Create giữ aria-disabled cho tới khi được tích, có gắn lý do (C8).
3. **Một 409 `rule_overlap`** từ preview hoặc lưu (khi có engine phục vụ catalog) được hiện thành:
   - "This would make two masks decide <coordinate> differently: <rule A> and <rule B>.";
   - link tới cả hai rule;
   - `error` của server trong Details.

   Chồng lấn ở đây do server từ chối, nên không có checkbox.

**Kiểm thử.**

- `crates/egret-perm-compile/tests/routes.rs`:
  - preview một mask theo path trên một cột mà một mask theo tag đã chạm tới, **trên một catalog không engine nào phục vụ** (`orphan_catalog`), trả `effect.alreadyMasked` nêu tên rule theo tag;
  - cùng preview đó cho PATCH của chính rule thì không liệt kê chính nó.
- `FE/routes/governance/__tests__/ruleEffect.test.ts`: ba dòng của bảng trên, với fixture mà các principal chưa sắp xếp sẵn.
- `__tests__/RuleDialog.test.tsx`: cảnh báo, các dòng theo nhóm, checkbox chặn Create, và cách hiện 409.

### C7. Preview nói bằng tác dụng (N5)

**Hôm nay.**

- `PreviewResponse` là `{affected: [{id, name}], compile: {services, created, updated, deleted, unchanged, skipped}}` (`crates/egret-perm-compile/src/routes.rs:953-1010`).
- Hộp thoại in "Would recompile <tên|no profiles>" và bốn con số (`RuleDialog.tsx:473-485`).
- Handler preview chạy `write_rule` và `name_profiles` trong một transaction được rollback (`routes.rs:2061-2085`).
- Engine theo catalog đến từ `registry.engine_catalog_attachments` ⋈ `engine_mgmt.engines` với `authz_service_id IS NOT NULL` (`services_for_catalog_via`, `crates/egret-policy/src/store.rs:1487`). Không hàm nào đếm engine.
- Thành viên miễn trừ được phân giải lúc compile bởi `expand_enumerated_principals` (`crates/egret-perm-compile/src/members.rs:678`). Các truy vấn thành viên chỉ đọc là `group_members_via` (`:552`), `workspace_members_via` (`:524`) và `role_holders_via` (`:496`).

**Mới, backend (B3).** `PreviewResponse` có thêm `effect: Option<RuleEffect>`, có mặt với preview rule và vắng mặt với preview grant. Nó được tính từ `NewRule` **trước** `write_rule`, trong cùng transaction, loại trừ `id` của chính rule khi là PATCH. Hình dạng trên dây (camelCase):

```text
RuleEffect {
  kind: "mask" | "row_filter",
  catalogs: [ { catalogId, catalogName,
                engines: [ { id, name, engineType } ],      // engine gắn vào catalog VÀ gắn với một profile
                notEnforced: null | "no_engine" | "no_profile" } ],
  tableCount: number | null,     // null khi một cấp đường dẫn là mẫu
  columnCount: number | null,    // chỉ cho mask; null với mẫu hoặc lọc dòng
  enforcedBy: number,            // số engine khác nhau trên các catalog
  notEnforced: null | "no_assignment" | "no_engine" | "no_profile",   // cho cả rule; null khi enforcedBy > 0
  exemptMembers: number,         // số người khác nhau trong exempt.include (đã khai triển nhóm, role, workspace; `public` tính 0 và bật exemptEveryone)
  exemptEveryone: boolean,
  alreadyMasked: [ { ruleId, ruleName, maskType, coordinate, exempt: PrincipalSet } ]   // C6, chỉ mask, ≤ 20
  truncated: boolean,            // alreadyMasked có thể chưa đủ: một giới hạn đã cắt nó (xét 50 cột, đọc 1000 classification, 20 mục)
}
```

- **Phạm vi của rule theo tag.** Các dòng `catalog_tag_assignments` của key classification, join với `registry.catalogs` theo `warehouse_id`. Dòng nào có warehouse không catalog nào nêu tên thì bỏ qua ở đây; Where used đã đếm chúng là `orphaned`.
- **Phạm vi của rule theo path.** Catalog của scope. `tableCount` là 1 khi `schema` và `table` được nêu không có ký tự glob (`*`, `?`), ngược lại là `null`. `columnCount` theo cùng quy tắc cho `column`. Một mask không có cột nghĩa là "mọi cột": `columnCount` là `null`, và UI nói "every column of <bảng>" (xem C8).
- **`notEnforced`.** Theo catalog: `"no_engine"` khi catalog không gắn engine nào, `"no_profile"` khi có engine gắn nhưng không engine nào gắn với profile, `null` khi có engine thực thi rule ở đó. Cho cả rule: `"no_assignment"` khi rule không chạm tới catalog nào (classification không được gán ở nơi nào có registry catalog), ngược lại `null` khi `enforcedBy > 0`, ngược lại `"no_profile"` nếu có catalog chạm tới mang lý do đó, còn lại `"no_engine"`.
- **Phạm vi của `alreadyMasked`.** Xét cả cột chính xác, mẫu cột và cả bảng (mask theo path không có cột, trên một bảng nêu đích danh). Với mask theo path đã lưu, các cấp phải khớp; hai mẫu cột được coi là chồng nhau khi một mẫu khớp mẫu kia. Với mask theo tag đã lưu, các classification gán cho các cột chạm tới, và cho cả bảng, được so theo tên, phân biệt hoa thường. Một rule theo tag xét 50 cột đầu; `truncated` cho biết khi một giới hạn đã áp dụng.

**Mới, frontend.**

1. Khối preview mở đầu bằng một câu do `effectSentence(effect)` (`ruleEffect.ts`) dựng, ví dụ:
   - "Masks 1 column on 1 table · enforced by 2 engines · 3 people exempt"
   - "Masks every column of 1 table" (mask theo path không có cột)
   - "Masks the columns matching the path" (một mẫu)
   - "Filters rows of 4 tables · … · nobody exempt"
   - "Masks 12 columns on 5 tables · enforced by 1 engine · 1 person exempt"
2. **`enforcedBy === 0`** là một cảnh báo, không phải chú thích: "**Not enforced yet.** <catalog> has no engine attached, so this rule is stored but no query is masked. Attach an engine in Control Plane > Engines." Với `no_profile`, câu cuối thành: "Its engines have no policy profile; bind one in Governance > Engine & policy."
3. Danh sách profile và các con số compile chuyển vào một `<details>` có nhãn "What egret will recompile". Các dòng `skipped` cũng ở đó.

**A11y.** Cảnh báo dùng một vùng `role="status"` gắn vào nút Create bằng `aria-describedby`, để trình đọc màn hình đọc nó khi Create nhận focus.

**Kiểm thử.**

- `crates/egret-perm-compile/tests/routes.rs`:
  - (a) catalog của fixture (một engine được gắn) cho `enforcedBy == 1` và `notEnforced == null`;
  - (b) `orphan_catalog` cho `enforcedBy == 0` và `notEnforced == "no_engine"`;
  - (c) miễn trừ một nhóm 2 người cộng trực tiếp một người trong số đó cho `exemptMembers == 2`;
  - (d) một rule theo tag cho `columnCount == 1` với cột có tag duy nhất của fixture.
- `ruleEffect.test.ts`: các biến thể câu tác dụng.
- `RuleDialog.test.tsx`: cảnh báo, và phần mở rộng details chứa tên các profile.

### C8. Hộp thoại rule: lý do, ô bắt buộc, mặc định, bản nháp, lỗi luôn trong tầm nhìn (N6, N11, N12, N26)

**Hôm nay.**

- **Create bị vô hiệu.** Create là `disabled` (`RuleDialog.tsx:525`, `disabled={!canSave || saving}`), nên nó rời thứ tự tab (N22). Khi có `blockedReason`, lý do là một `Alert` phía trên hộp preview (`:447-451`). Khi chỉ thiếu preview, lý do là chữ xám trong hộp preview (`:507-512`). Không lý do nào gắn với nút.
- **Thiếu cột.** Lượt đi thử ghi nhận "Mask + Path silently disabled because Column isn't marked required". Code nói khác: không có cột là hợp lệ và nghĩa là *mọi cột* (`ensure_scope_levels_are_named` bỏ qua các cấp vắng mặt, `crates/egret-perm-compile/src/grants.rs:445-460`, và `scope_to_resources` in chúng thành `*`). Lỗi thật là hộp thoại không bao giờ nói điều đó.
- **Mặc định.** Kind mặc định là `mask` (`:214`) và Applies to mặc định là `tag` (`:224`). Tab Masks của Data ghi đè cả hai bằng selector path và `row_filter` trên một bảng (`MasksFiltersTab.tsx:245-246`). Đó là "Filter rows + Path" mà lượt đi thử gặp.
- **Mất nháp khi Cancel.** Cancel unmount form (`RuleDialog.tsx:180-186`, `{open && <RuleForm/>}`), mất hết mọi ô.
- **Lỗi khuất tầm nhìn.** `saveError` hiện ở đầu thân cuộn (`:347-352`). `previewError` hiện trong hộp preview ở cuối (`:467-471`). Cái nào cũng có thể nằm ngoài màn hình, và các ô bị cuộn đi mất (N26).

**Mới.**

1. **Applies to** thay cho "Selects by", với hai lựa chọn **Classification** và **Path** (C1).
   - Ô bắt buộc mang chữ "(required)" hiện rõ trong `Label` và `aria-required="true"`: Name; biểu thức của Custom và Row filter; Classification khi Applies to = Classification; Catalog khi Applies to = Path.
   - Với mask theo path, chữ trợ giúp của ô Column ghi "Leave empty to mask every column of the table."
2. **Create không bao giờ bị vô hiệu im lặng.**
   - Create được vẽ `aria-disabled` (không phải `disabled`) khi bị chặn, nên vẫn giữ focus (mẫu `ReasonButton`, `FE/components/workflows/ReasonButton.tsx`). Nhấp khi bị chặn không làm gì.
   - `aria-describedby` của nó trỏ vào một dòng lý do vẽ **ở footer, cạnh nút**: cái đầu tiên trong số `blockedReason`, "Preview the rule to enable Create." (chưa preview, hoặc form đã đổi từ đó), lý do của checkbox C6, hoặc tóm tắt lỗi preview.
   - Nút Preview theo cùng mẫu.
3. **Mặc định theo lối vào.**

   | Lối vào | Kind | Applies to | Điền sẵn |
   |---|---|---|---|
   | Trang classification (dải Effect, trạng thái trống và thanh công cụ của tab Rules) | Mask, Redact | Classification | classification này; tên "Mask <tên>" |
   | Data, bảng, tab Rules | **Mask**, Redact | Path | catalog/schema/table; Column để trống kèm trợ giúp "mọi cột", được focus |
   | Data, panel cột (nút mới "Add rule", `policy:update`) | Mask, Redact | Path | cột đó; khi cột mang classification, gợi ý "Or apply it to <classification>" chuyển Applies to |
   | Trang rule, Edit | của rule | của rule | rule |

   Row filter vẫn chỉ cách một lần nhấp. Mặc định của bảng đổi từ `row_filter` sang `mask` vì việc trong lượt đi thử, và cả tên khu vực, đều xoay quanh việc che. Lý do hiện tại của `row_filter` (selector path nêu một bảng là đủ cho một row filter, `RuleDialog.tsx:163-165`) được giữ cho kind **Row filter**: chuyển sang kind đó từ lối vào bảng sẽ điền scope.
4. **Bản nháp sống sót qua Cancel.**
   - Hộp thoại giữ trạng thái form trong một kho ở cấp module, khoá theo người dùng đã đăng nhập và lối vào (`FE/routes/governance/ruleDraft.ts`: `ruleDraftKey(userId, entry)`, `loadRuleDraft`, `saveRuleDraft`, `clearRuleDraft`). Nó chỉ ở trong bộ nhớ, nên sống qua một đường vòng trong SPA (ví dụ gỡ classification cấp bảng trong Data) nhưng không qua lần tải lại. Nó không phải `localStorage`, thứ FE-DATA-3 để dành cho tuỳ chọn.
   - Đóng một form có thay đổi (Cancel, Escape, nhấp ra ngoài) sẽ hỏi **"Discard this rule?"** với **Keep editing** và **Discard**. Discard xoá bản nháp.
   - Mở lại cùng lối vào sẽ khôi phục bản nháp, kèm dòng "Restored your unsaved rule · Start over".
   - Create thành công thì xoá bản nháp.
   - `useUnsavedChangesWarning(dirty)` lo trường hợp tải lại (FE-ACTION-5).
5. **Lỗi ở cùng form.**
   - Lỗi preview và lỗi lưu hiện ở vùng footer cạnh Create. Vùng đó là `sticky bottom-0` trong thân cuộn, nên luôn trong tầm nhìn.
   - Focus chuyển tới tiêu đề của lỗi.
   - Các ô giữ giá trị (FE-ACTION-3).
   - Danh sách gợi ý nhóm của `PrincipalPicker` hiện bên dưới dòng trợ giúp thay vì đè lên nó (N26). Đây là thay đổi cục bộ ở chế độ exempt của `PrincipalPicker`, kiểm bằng một assertion về thứ tự DOM.

**Kiểm thử.** `__tests__/RuleDialog.test.tsx` có thêm:

- lý do ở footer là mô tả của nút, với nút focus được và `aria-disabled`;
- "(required)" trên mỗi nhãn bắt buộc;
- chữ trợ giúp của mask theo path;
- mặc định theo lối vào, mỗi dòng một test;
- xác nhận bỏ, và khôi phục sau Cancel → mở lại;
- vùng lỗi nhận focus.

Playwright (Task 12) kiểm footer vẫn trong tầm nhìn sau một lỗi, ở 1440 và 390 px (FE-TEST-2).

### C9. Clearance nói nó làm gì trước lần thêm đầu tiên, và ai đang được cấp (F3, N7, N21)

**Hôm nay.**

- Tab nói đối tượng "can only be opened by principals holding one of the required roles or belonging to one of the required groups" (`FE/routes/governance/classifications/tabs/ClearanceTab.tsx:145-148`).
- Giới hạn thực thi chỉ hiện khi đã có nhóm hoặc role (`:163-196`).
- Bảng trống ghi "Not gated — anyone can open objects with this classification." (`:357-361`).
- Các cảnh báo là `Badge variant="warning"`, tức `bg-yellow-600 text-white` (`FE/components/ui/badge.tsx:22-23`), một class palette thô. Lượt 2 tính được khoảng 2.9:1, không đạt AA (N21).
- Không endpoint nào trả về những người mà một clearance cho phép. Các phần cần thiết đã có:
  - người giữ role theo **tên** (`role_holders_via`, `members.rs:496`);
  - thành viên nhóm theo tên (`group_members_via`, `:552`);
  - `GET /groups/{id}/members` (`crates/egret-controlplane/src/access/mod.rs:745`, `group:read`).
- Clearance lưu role theo tên và nhóm theo **id** (`migrations/0082_classification_policies.sql`, `0083`).

**Mới.**

1. **Phạm vi trước tiên, luôn hiện** (trước bất kỳ lần thêm nào). Một `Callout` ngắn dùng token (xem 3):
   - "**Clearance decides who may open data with this classification** in egret (tables, columns, BI objects).
   - On engines it is enforced only through **required roles**: their members are named in each engine's policy. A required **group** is enforced when the data is opened in egret, not in SQL Lab or notebooks.
   - To hide values from everyone without clearance, add a **mask rule** as well."

   "add a mask rule" là nút mở `RuleDialog` như ở C2. Các cảnh báo đã đo sẵn có (không chặn ghi, view, giới hạn 500 cột của StarRocks) chuyển vào một `<details>` "Engine details", giữ nguyên nội dung.
2. **Ai đang được cấp (B4).** Một lệnh đọc mới `GET /api/control-plane/perm/clearances/{tag_id}/people` (`policy:read`; nó lộ thông tin người, thứ mà `/perm/people` cũng chặn bằng `policy:read`). Nó trả về:

   ```text
   ClearedPeople {
     count: number,
     people: [ { userId, label, username: string | null,
                 via: [ { kind: "role" | "group", name } ],
                 onEngines: boolean } ],      // true khi một required role nêu tên họ, tức engine thực thi cho họ
     truncated: boolean                         // people tối đa 200, count luôn chính xác
   }
   ```

   - Người giữ role là người dùng có grant `allow` còn hiệu lực của một required role (`role_holders_via`).
   - Thành viên nhóm là `user_groups` của các id nhóm bắt buộc.
   - Kết quả bỏ trùng theo id người dùng và sắp theo label.
   - Một tag không có clearance trả `{count: 0, people: [], truncated: false}`. Tag không tồn tại trả 404.

   Tab hiện "**N people are cleared**" với một danh sách mở rộng được (`Collapsible`/`<details>`): tên, username, qua đâu, và dấu "engines" khi `onEngines`. Không có `policy:read` thì hiện số role và số nhóm, kèm câu "Seeing who is cleared needs the policy:read permission."
3. **Độ tương phản (N21).** Các cảnh báo thôi dùng `Badge variant="warning"`. Badge là component dùng chung nên không bị sửa (FE-TEST-3); palette thô của nó là nợ đã ghi trong rulebook. Một `Callout` cục bộ dùng `bg-highlight/15 text-foreground border-highlight`, với `AlertTriangle` màu `text-foreground`. `--highlight` và `--foreground` là token (`FE/index.css`), nên không có màu thô nào (FE-VISUAL-1). Chữ thân trên nền pha 15% `--highlight` trên `--background` giữ độ tương phản của foreground (trên 7:1 ở cả sáng và tối). Bước Playwright đo nó bằng `getComputedStyle` (FE-TEST-2).
4. "Not gated — …" thành "None: anyone who can read the data can open it." (theo bảng thuật ngữ).

**Kiểm thử.**

- `crates/egret-perm-compile/tests/people.rs`, `cleared_people_*`: người giữ role, thành viên nhóm, và một người thuộc cả hai được đếm một lần; grant hết hạn không được đếm; `onEngines` chỉ true khi qua role; chặn bằng `policy:read` (403 với `nobody`); 404 với tag không tồn tại.
- `classifications/__tests__/ClearanceTab.test.tsx`: chữ phạm vi khi clearance trống, số lượng và danh sách, chữ khi thiếu quyền, và nút mask rule mở hộp thoại.

### C10. Tạo và sửa classification: trùng tên, quy tắc key, nhãn màu (N8, N9, N10)

**Hôm nay.**

- **Bộ ký tự của key.** Server chỉ nhận key gồm `[a-z0-9:-]` (`crates/egret-bi/src/tags.rs:52-65`, một vòng lặp theo ký tự). Gợi ý trên UI hứa cả `_` (`ClassificationsIndex.tsx:398-401`), và `slugifyTagKey` giữ `_` (`tagParts.tsx:8-16`). Chỉ kiểm khi gửi.
- **Trùng key.** Key trùng được ánh xạ thành `BiError::Conflict("a tag with that key already exists")` (`tags.rs:40-47`) → `CpError::Conflict` → 409 `{"error":"conflict: a tag with that key already exists"}` (`crates/egret-controlplane/src/error.rs:118,148-170`), không có `code`. Hộp thoại hiện mọi 409 dưới ô Key, nguyên văn (`ClassificationsIndex.tsx:325`).
- **Tên hiển thị.** Tên không duy nhất ở đâu cả: không có index (`migrations/0080_tags.sql:6-14`) và không có kiểm tra. **Điều này ảnh hưởng tới việc thực thi, không chỉ tới sự rõ ràng:** engine khớp rule theo classification bằng **tên** của classification (`ruleMatch.ts:55-64`; `TagRulesSection.tsx:80-81`). Hai classification cùng tên "Public" vì thế dùng chung mọi rule viết trên một trong hai.
- **Màu.** Ô màu là `type="color"` gốc của trình duyệt, có nhãn "Colour" hiện rõ (`ClassificationsIndex.tsx:368-376`, `TagHeader.tsx:129-138`). Lượt 2 nghe nó được đọc là "textbox" (N22).

**Mới (B1).**

1. **Tên trùng bị từ chối, không phân biệt hoa thường,** khi tạo và khi đổi tên. Các handler controlplane `create_tag` và `update_tag` (`crates/egret-controlplane/src/tags.rs:167,201`) gọi hàm mới `TagRepo::name_taken(name, except: Option<Uuid>) -> Result<Option<ExistingTag>, BiError>` (`ExistingTag { id, key, name }`; so khớp `lower(name) = lower($1)`). Khi trùng, trả 409:

   ```json
   {"error":"A classification named \"Public\" already exists (key public).","code":"tag_name_taken",
    "name":"Public","existing":{"id":"…","key":"public","name":"Public"}}
   ```

   Việc kiểm nằm trong handler HTTP, không nằm trong `TagRepo::create`. Hơn 50 fixture test tạo classification tên "PII" qua repo trong một test database dùng chung, và chốt này là quy tắc sản phẩm cho người soạn, không phải bất biến của bảng (§9, §10).
2. **Lỗi key có mã.**
   - Key trùng thành 409 `{"error":"A classification with the key \"pii\" already exists (PII).","code":"tag_key_taken","key":"pii","existing":{"id","name"}}`. `TagRepo::create` ánh xạ lỗi vi phạm unique thành `BiError::Coded`, đọc `existing` sau đó; `map_conflict_err` bị bỏ.
   - Key không hợp lệ thành 400 `{"error":"…","code":"tag_key_invalid","key":"…","allowed":"a-z 0-9 : -"}`.
3. **Kiểm ngay trong hộp thoại.**
   - Ô key kiểm khi gõ theo `TAG_KEY_PATTERN = /^[a-z0-9:-]+$/` (xuất từ `tagParts.tsx`; quy tắc của server được viết một lần ở phía này). Gợi ý ghi "Lowercase letters, digits, `-` and `:`."
   - `slugifyTagKey` đổi `_` thành `-`.
   - Ô tên kiểm danh sách đã tải (`useTags`) không phân biệt hoa thường, và hiện "A classification named Public already exists." kèm link tới nó trước khi gửi. `tag_name_taken` của server là phán quyết cuối và hiện cùng câu.
   - `tag_key_taken` hiện "Key pii is taken by <tên>." dưới ô Key.
   - Lỗi dùng `aria-invalid` và `aria-describedby` (FE-FORM-4).
4. **Màu.** Ô giữ `type="color"` và `Label` của nó, thêm `aria-describedby` với chữ "Colour of the swatch shown next to the name." Ô màu xem trước bên cạnh hiện giá trị hex bằng chữ, để đọc được mà không cần mở bộ chọn. Cách đọc "textbox" đến từ control gốc của trình duyệt; nhãn đã đúng và được giữ.

**Kiểm thử.**

- `crates/egret-controlplane/tests/tags_counts.rs` (file này đã dựng router controlplane với PDP thật):
  - tạo "PII" rồi "pii" cho 409 `tag_name_taken` kèm `existing.key`;
  - đổi tên một classification khác thành "PII" cho cùng kết quả;
  - đổi tên một classification thành chính tên nó với hoa thường khác cho 200;
  - key trùng cho `tag_key_taken`;
  - `bad_key` cho 400 `tag_key_invalid`.
- `crates/egret-bi/tests/tags_db.rs`: assertion về key trùng đổi từ `BiError::Conflict(_)` sang `BiError::Coded { .. }` kèm mã.
- `classifications/__tests__/ClassificationsIndex.test.tsx`: lỗi key tại chỗ với `a_b`, slug của "a_b c" là `a-b-c`, cảnh báo trùng tên trước khi gửi, và mỗi mã của server nằm cạnh đúng ô của nó.

### C11. Luồng xoá và gỡ: một quy ước (N13, N14, N15)

**Hôm nay.**

- **Xoá classification** yêu cầu gõ key, phân biệt hoa thường (`FE/routes/governance/classifications/DeleteTagDialog.tsx:113-120`). Nó báo trước sẽ bị từ chối ("1 rule selects it: the delete will be refused …", `:80-84`) mà vẫn để nút Delete bật. Khi gặp 409, nó toast câu của server, câu này nêu `perm.rules` (`crates/egret-bi/src/tags.rs:392-400`), và chỉ tới một mục bằng tên (`FE/routes/governance/tagDeleteFailure.ts:22-26`).
- **Xoá rule** yêu cầu gõ "delete" (`FE/routes/governance/people/RulePage.tsx:72-81`, `confirmText` mặc định của `ConfirmDeleteDialog`). Xoá xong nó ở lại trang, và trang lúc đó ghi "No rule has this id" (`:41-45`).

**Quy ước mới, ghi trong bảng thuật ngữ.**

- **Delete** loại bỏ một định nghĩa (một classification, một rule). Nó dùng `ConfirmDeleteDialog`, trong đó bạn **gõ tên** hiện ở tiêu đề. Một quy tắc cho cả hai: tên classification (không phải key); tên rule. Không phân biệt hoa thường, theo mặc định của component.
- **Remove** gỡ một lần gán khỏi một đối tượng, hoặc gỡ một người khỏi clearance. Đó là một `AlertDialog` một bước nêu điều gì sẽ thôi áp dụng (C4), hoặc không có hộp thoại với các dòng clearance (giữ nguyên).

Cụ thể:

1. **Xoá classification.**
   - Khi có rule đang dùng classification, nút Delete của hộp thoại là aria-disabled với lý do "Delete or change these rules first:", tiếp theo là link tới từng rule. Ô gõ để xác nhận không hiện.
   - Có `policy:read` thì danh sách rule lấy từ `GET /perm/rules`. Không có quyền đó thì 409 của server là chốt chặn, và thân của nó giờ mang theo danh sách rule (bên dưới).
   - **B1**: `TagRepo::delete` đọc `SELECT id, name FROM perm.rules WHERE tag_key = (SELECT key FROM tags WHERE id = $1)` trước, rồi từ chối với 409 `{"error":"Rules still use this classification: <tên>. Delete them or change what they apply to first.","code":"tag_in_use","rules":[{id,name}]}`. Ánh xạ khoá ngoại vẫn giữ làm dự phòng, cùng mã và `rules: []`.
   - Toast (khi vẫn xảy ra: race, hoặc không có `policy:read`) ghi "<tên> is still used by <n> rules." với hành động **Show rules**. `perm.rules` không bao giờ xuất hiện.
2. **Xoá rule.** Gõ tên rule. Thành công thì chuyển tới nơi trang rule được mở từ đó (`from` của C1, mặc định `/governance/classifications`), và toast "Rule <tên> deleted." (FE-ACTION-2 chỉ cho toast khi chuyển trang nếu phản hồi chính là điều cần thấy; ở đây toast xác nhận một hành động mà trang của nó đã biến mất, nên được giữ, giống `DeleteTagDialog` khi gọi từ trang danh sách).
3. `tagDeleteFailure.ts` dựa vào mã, không dựa vào status, và lời văn theo bảng thuật ngữ.

**Kiểm thử.**

- `classifications/__tests__/DeleteTagDialog.test.tsx`: bị vô hiệu kèm link tới các rule, xác nhận bằng cách gõ tên, và toast cho `tag_in_use` khi không có `policy:read`.
- `people/__tests__/RulePage.test.tsx`: gõ tên, rồi chuyển tới `from`.
- `crates/egret-bi/tests/tags_db.rs`: `deleting_a_tag_a_live_rule_selects_is_refused_not_silently_cascaded` kiểm `code == "tag_in_use"` và id của rule nằm trong `rules`.

### C12. Explain: phán quyết trước, phần nội bộ thu gọn (N19; phần còn lại thuộc Gói 2)

**Hôm nay.** `ExplainResultView` vẽ các khối phán quyết, rồi một `Alert` luôn mở gồm tối đa năm đoạn (`FE/components/perm/ExplainResult.tsx:1209-1374`): cơ chế danh sách tag, chiều của enricher, role và usersync, giả định, mask không được báo, polling.

**Mới.**

- Phán quyết vẫn đứng đầu.
- **Một dòng hiện rõ** mang giới hạn làm đổi nghĩa câu trả lời: "Masks on Trino and row filters are not shown here, so *Allowed* can still return masked values or filtered rows."
- Mọi thứ khác chuyển vào `<details><summary>How this answer was worked out</summary>…</details>`, chữ giữ nguyên.
- Trạng thái "Nothing is enforcing" giữ huy hiệu của nó. Đoạn văn rút gọn thành: "No engine with a policy profile serves this catalog, so nothing is enforced here (this is not a refusal). Attach an engine to govern it." Câu dài hiện tại chuyển vào details.

Component này dùng chung cho Check access và Who can read, nên cả hai cùng đổi. Đó là chủ ý (một câu trả lời explain, một cách vẽ).

**Kiểm thử.** `FE/components/perm/__tests__/ExplainResult.test.tsx` (bộ test có sẵn): dòng ngắn hiện rõ, các đoạn văn nằm trong một `details` đang đóng, và các assertion về lời văn của những đoạn bị chuyển vẫn tìm thấy chúng sau khi mở.

### C13. Responsive, a11y, 403 (N20, N22, N23)

1. **Danh sách Classifications ở 390 px.** Các cột Key, Description và BI objects nhận `className: "hidden md:table-cell"` (FilterableTable áp `className` của cột cho cả header và ô: `FE/components/explore/FilterableTable.tsx:237,240,271`). Name, Data, Clearance và các hành động được giữ. Key vẫn có trên trang classification, và ô tìm kiếm vẫn khớp theo key.
2. **Bảng cột của Data ở 390 px** (`OverviewTab.tsx:300-352`). Nó dùng `STACKED_TABLE` (`tabParts.tsx:62-67`), mẫu mà People đã dùng: mỗi dòng thành một thẻ và mỗi ô có nhãn qua `data-label` ("Type", "Classifications", "Rules").
3. **Dấu hiệu cuộn chung** và việc xuống dòng của dải tab thuộc U3 (KI-233/234 trong `docs/superpowers/plans/2026-10-21-known-issues-fix-programme.md`), không làm trùng ở đây.
4. **Vô hiệu kèm lý do (N22).** Mọi hành động bị vô hiệu mà gói này đụng tới đều dùng `aria-disabled` với lý do được gắn (mẫu C8): Preview và Create của hộp thoại rule, Delete của classification, Add classification, Add clearance. Các ô lưới không tên của cây schema nằm ngoài khu vực này (CatalogTree) và được liệt kê ở §11.
5. **403 (N23).**
   - `Forbidden` (`FE/routes/Forbidden.tsx:2-4`) có thêm các prop tuỳ chọn `{ reason?: string; back?: {to: string; label: string} }`, theo kiểu chỉ thêm (FE-TEST-3).
   - Nó hiện "You don't have access to this page.", lý do, và một link quay lại. Link mặc định là "Go to the home page" → `/`.
   - `RequireGovernanceAccess` (`FE/auth/guards.tsx:91-94`) truyền lý do "Governance needs a governance permission such as tag:read or policy:read.", và `RequirePermission` truyền quyền mà nó đã kiểm.
   - Vẽ 403 **bên trong** khung ứng dụng, và một cách diễn đạt cho mọi trang, là KI-230/231 (U3). Gói này không di chuyển guard.
6. **Tab của trang tag ở 390 px.** Hôm nay chúng xuống thành 3 hàng. Với các nhãn ngắn hơn của C1, chúng vừa 2 hàng. Đo trong Task 12.

**Kiểm thử.** Vitest cho các prop của `Forbidden` và lý do của các guard (`FE/__tests__/governanceRouting.test.tsx`). Playwright ở 390 × 844 cho hai bảng: sự thật Name và Classifications hiện mà không cần cuộn ngang, và `document.scrollingElement.scrollWidth <= innerWidth`.

## 4. Bảng thuật ngữ

Một thuật ngữ cho một khái niệm. UI dùng cột **Thuật ngữ (UI)**. Cột tiếng Việt dành cho tài liệu tiếng Việt.

| Khái niệm | Thuật ngữ (UI) | Định nghĩa hiện trong chữ trợ giúp | Tên đã bỏ | Tiếng Việt (tài liệu) |
|---|---|---|---|---|
| Nhãn gắn lên dữ liệu | **Classification** (số nhiều Classifications) | A label you attach to tables and columns (and BI objects) to say what kind of data they hold. | tag, Tags, "Pick a tag", "A tag to classify…" | Phân loại |
| Định danh ổn định của nó | **Key** | Fixed once created; lowercase letters, digits, `-` and `:`. | "stored as", "tag key" | Khoá |
| Đặt nó lên dữ liệu | **Attach** / **Remove** | Attach a classification to a table or column; remove it from one. | assign, unassign, add tag | Gắn / Gỡ |
| Điều một rule làm với dữ liệu | **Rule**, loại **Mask** hoặc **Row filter** | A rule masks a column's values or filters a table's rows for everyone except the people it exempts. | egret rule, Mask & filter, Masks & filters, Masking rules, MASK RULE, permission rule (perm.rules), policy | Quy tắc (Che / Lọc dòng) |
| Cách một mask che | **Mask type**: Redact, Partial (last 4), Partial (first 4), Hash, Nullify, Unmasked, Year only, Custom | — | `MASK`, `MASK_HASH` … trong danh sách | Kiểu che |
| Người mà rule không áp dụng | **Exempt** | People who see the real value (or every row). | Include, "except group" (chỉ giữ trong câu) | Miễn trừ |
| Rule được viết trên cái gì | **Applies to**: Classification hoặc Path | A rule applies to every column carrying a classification, or to one catalog path. | Selects by, Reaches this tag, Tagged: on column, by tag | Áp dụng cho |
| Ai được mở dữ liệu đã phân loại | **Clearance** | The roles and groups allowed to open data with this classification. | Required roles and groups, gated, Not gated, opened by principals, gate | Quyền truy cập theo phân loại |
| Nó được gắn ở đâu | **Where used** | The tables, columns and BI objects that carry it. | Carries it | Nơi dùng |
| Policy viết trong trình sửa của chính engine | **Engine policy** (tab "Engine policies") | A policy written directly in an engine profile's editor (Governance > Engine & policy), not generated from a rule or a grant. | Hand-written, Hand-written policies | Chính sách trong engine |
| Kiểm ai đọc được | **Who can read?** | Asks each engine's policy whether a person can read this. | Explain (giữ làm tên API), per policy profile | Ai đọc được? |
| Loại bỏ một định nghĩa | **Delete** (gõ tên để xác nhận) | — | "type delete", "type the key" | Xoá |
| Gỡ một lần gán | **Remove** (xác nhận, không phải gõ) | — | unassign, x | Gỡ |
| Engine thực thi một rule | **Enforced by N engines** / **Not enforced yet** | — | "Would recompile no profiles", "Nothing is enforcing" (chỉ giữ trong Explain) | Được N engine thực thi / Chưa được thực thi |

## 5. Những gì giữ nguyên

- Khoá URL và route: `/governance/classifications/:id?tab=…`, `/governance/rules/:id`.
- Key không đổi được.
- Danh sách hệ quả khi xoá.
- Cách nhóm của Where used.
- "from table" với classification kế thừa.
- Chốt preview trước Create.
- Explain là nơi duy nhất trả lời ai đọc được (F10 của #2).

## 6. Thay đổi backend (liệt kê)

| # | Thay đổi | Ở đâu | Trên dây |
|---|---|---|---|
| B1 | **Lỗi có mã cho các lần ghi classification.** `BiError::Coded { status, body }`, `CpError::Coded(StatusCode, Value)` và `PermHttpError::Coded { status, body }`, mỗi cái trả nguyên thân. Các mã: `tag_key_invalid` (400), `tag_key_taken` (409), `tag_name_taken` (409, kiểm trong `create_tag`/`update_tag` qua `TagRepo::name_taken` mới), `tag_in_use` (409 từ `TagRepo::delete`, kèm `rules`). | `crates/egret-bi/src/{error.rs,tags.rs}`, `crates/egret-controlplane/src/{error.rs,tags.rs}`, `crates/egret-perm-compile/src/routes/error.rs`; khối match vét cạn trong `crates/egret-automation/src/render/payload.rs:347-361` có thêm nhánh | `{error, code, …fields}`; câu `error` được giữ |
| B2 | Thân **`table_tag_mask_conflict`** từ cả ba đường ghi; `UnreadableTableMask` có thêm `tag_id`, `warehouse_id`, `more_tables`. | `crates/egret-policy/src/unreadable_table_mask.rs`, `crates/egret-perm-compile/src/routes.rs:1477-1491`, `crates/egret-bi/src/catalog_tags.rs:119-136`, `crates/egret-bi/src/tags.rs:198-207` | C5 |
| B3 | **`PreviewResponse.effect: Option<RuleEffect>`**, tính bởi file mới `crates/egret-perm-compile/src/effect.rs` (`rule_effect(conn, id, &NewRule)`), dùng `egret_perm::eval::glob_match` để khớp path và tên (không phân biệt hoa thường ở các cấp path, phân biệt hoa thường với tên classification, như engine làm). | `crates/egret-perm-compile/src/{effect.rs,routes.rs}` | C7 |
| B4 | **`GET /api/control-plane/perm/clearances/{tag_id}/people`** (`policy:read`) → `ClearedPeople`. | `crates/egret-perm-compile/src/routes/people.rs`, route trong `routes.rs` | C9 |
| B5 | **`TableUsage.warehouseId`**. | `crates/egret-controlplane/src/governance_tag_usage.rs:56-65` | C4 |

Sau B3, B4 và B5: sinh lại `openapi.json` (`cargo run -q -p egret-bin --bin egret -- openapi > openapi.json`), rồi `npm run gen:api` trong `frontend/`, không bao giờ chạy `orval` trần (FE-DATA-1).

**Không thuộc Gói 1 (đã xem xét và từ chối): kiểm id warehouse trên `PUT …/resource-tags`.** Đã xác nhận rằng một PUT nêu một warehouse không tồn tại vẫn trả 200 và lưu một dòng mồ côi. `catalog_tag_assignments.warehouse_id` không có khoá ngoại (`migrations/0081_catalog_tag_targets.sql:11`), và `assign_resource_tag` (`crates/egret-bi/src/catalog_tags.rs:665-695`) không kiểm gì. Bản thân chốt chặn chỉ vài dòng. Nhưng năm file test ở cấp route đang PUT với id warehouse bịa ra sẽ cần một dòng `catalog.warehouses` thật, mà dòng đó cần một workspace và một storage connection: `egret-bi/tests/catalog_tags_routes.rs`, `egret-bi/tests/tag_rename_unreadable_pair.rs`, `egret-controlplane/tests/governance_browse.rs`, `egret-perm-compile/tests/routes.rs` và `egret-policy/tests/unreadable_table_mask_db.rs`. Đó không phải thay đổi một dòng. UI luôn gửi id warehouse của một catalog đã đăng ký. Đã liệt kê ở §11.

## 7. Hình dạng dây và DTO (frontend)

| Kiểu | File | Ghi chú |
|---|---|---|
| `RuleEffect`, `EffectCatalog`, `EffectEngine`, `ExistingMask`, `PreviewResponse.effect` | sinh ra (`FE/api/generated/model/`) | sau `npm run gen:api` |
| `ClearedPeople`, `ClearedPerson`, `ClearedVia`; hook `usePermClearedPeople(tagId)` | sinh ra | orval đặt tên hook từ tag `perm` + hàm `cleared_people`, như `list_overlaps` thành `usePermListOverlaps` |
| `TableUsage.warehouseId` | sinh ra | |
| `ApiErrorCode` = `"tag_key_invalid" \| "tag_key_taken" \| "tag_name_taken" \| "tag_in_use" \| "table_tag_mask_conflict" \| "rule_overlap"` và các hàm thu hẹp `tableTagMaskConflict(e)`, `ruleOverlap(e)`, `tagInUse(e)`, `tagNameTaken(e)`, `tagKeyTaken(e)`, `tagKeyInvalid(e)` | `FE/routes/governance/apiErrors.ts` (tạo mới) | đọc `ApiError.code` và `ApiError.body` (`FE/api/client.ts:4-27`) |
| `RuleEntry` = `{kind: "classification", tagKey, tagName}` \| `{kind: "table", catalogId, schema, table}` \| `{kind: "column", catalogId, schema, table, column, classifications}` \| `{kind: "edit", rule}`; `seedRule(entry)`, `ruleDraftKey(userId, entry)` | `FE/routes/governance/ruleDraft.ts` | điều khiển mặc định của C8 và khoá bản nháp |

## 8. Tóm tắt a11y và responsive

- Mọi hành động bị vô hiệu đều có control `aria-disabled` focus được và một lý do hiện rõ gắn qua `aria-describedby` (FE-ACTION-4, N22).
- Ô bắt buộc mang "(required)" và `aria-required`.
- Chip có hai điểm focus có tên. Ô màu `aria-hidden`.
- Cảnh báo chỉ dùng token (FE-VISUAL-1) và được kiểm độ tương phản ở chế độ sáng và tối trong trình duyệt (FE-VISUAL-4, FE-TEST-2).
- 390 px: dải Effect xếp chồng; danh sách ẩn ba cột ít ưu tiên; bảng cột của Data xếp thành thẻ; footer của hộp thoại dính đáy.

## 9. Migration và tương thích

- **Không có migration cơ sở dữ liệu.** Tính duy nhất của tên là kiểm trong handler (C10). Các thay đổi backend khác là phía đọc hoặc thân lỗi.
- **Thân lỗi vẫn tương thích.** Mọi thân có mã giữ câu `error` mà frontend và script đang đọc. Một script khớp `"conflict: a tag with that key already exists"` sẽ thấy `"A classification with the key \"pii\" already exists (PII)."` thay vào đó: chữ cũ không được giữ, vì chưa có gì triển khai cho người dùng thật (phán quyết ngày 2026-10-20 trong spec Ranger).
- **Tên trùng sẵn có** (nếu một bản cài nào đó có) vẫn chạy. Đổi một trong số chúng sang tên còn trống được cho phép; đổi sang một tên đã có thì bị từ chối. Một mục cảnh báo trên Overview cho tên trùng sẵn có không thuộc gói này (§10).
- **`PreviewResponse.effect` là tuỳ chọn,** nên `GrantDialog` của Engine & policy, nơi preview grant, không bị đụng.
- **Khoá URL của tab** không đổi.

## 10. Rủi ro

- **Race khi kiểm tên.** Hai lần tạo đồng thời cùng một tên có thể cùng qua chốt kiểm trong handler. Chấp nhận: đây là chốt cho người soạn, và tác động của một tên trùng chính là tác động đang tồn tại hôm nay.
- **Chi phí của `effect`.** Phạm vi của một rule theo tag đọc mọi lần gán của classification đó cộng mọi mask rule. Nó có giới hạn: `alreadyMasked` tối đa 20 và các toạ độ được bỏ trùng. Nó chỉ chạy khi Preview, không chạy khi đọc danh sách.
- **Phát hiện chồng lấn ở mức mô hình (C6) có thể cảnh báo thừa.** Nó báo hai mask trên một cột kể cả khi compiler sẽ sắp thứ tự chúng rõ ràng (khác dải ưu tiên). Chấp nhận, vì nó chỉ cảnh báo và yêu cầu một dấu tích, còn 409 của server vẫn là lời từ chối.
- **Kho bản nháp trong bộ nhớ module** mất khi tải lại, điều đã được nói trong dòng khôi phục. Nó cũng có thể khôi phục bản nháp cho người dùng khác sau khi đổi phiên trong cùng tab, nên khoá bản nháp chứa id của người dùng đã đăng nhập (`me.user_id`); khoá của người khác không bao giờ khớp.
- **Phối hợp với U3** (chương trình known-issues): 403 trong khung ứng dụng, dấu hiệu cuộn và sidebar (KI-225, KI-230, KI-231, KI-233, KI-234) thuộc U3. Nếu U3 xong trước, C13.5 chỉ còn là truyền lý do vào component của U3.

## 11. Các lỗi liên quan đã tìm thấy (không sửa ở đây trừ khi có ghi)

1. **`PUT …/resource-tags` với id warehouse không tồn tại trả 200** và ghi một dòng mồ côi. DELETE cũng trả 200. Xem §6 về lý do không đưa vào gói này. Một `tag_id` không tồn tại vấp khoá ngoại và trả **500** "internal error" thay vì 400/404 (`crates/egret-bi/src/catalog_tags.rs:84-190` → `BiError::Db`).
2. **Clearance trên StarRocks đọc ra NULL thay vì từ chối** với người đọc chưa được cấp trên một cột bị mask (`ClearanceTab.tsx:20-21`, `COMPILES_TO`). Đây là hành vi sẵn có của engine, đã được nói trên UI. Không đổi.
3. **`Badge variant="warning"/"success"/"processing"` dùng class palette Tailwind thô** (`FE/components/ui/badge.tsx:20-25`), và chữ trắng trên `yellow-600` không đạt AA. Đây là component dùng chung (FE-TEST-3), nên được sửa riêng. Gói này thôi dùng nó trong tab Clearance.
4. **Các ô lưới của cây schema không có tên truy cập được** (N22, `FE/routes/governance/data/CatalogTree.tsx`). Ngoài khu vực này.
5. **Skeleton 3-5 giây** trên các tab (N24): một phần do backend khởi động lại trong lượt đi thử. Chưa đo; không xử lý.
6. **E2**: SSO bắt đầu từ `127.0.0.1` kết thúc ở một trang 400 thô "missing oidc state". Việc này thuộc tài liệu của demo stack (ngoài mục tiêu).
7. **`table_level_attachment_of` chỉ báo bảng đầu tiên** (`LIMIT 1`). C5 báo `moreTables` nhưng không báo danh sách.

## 12. Câu hỏi mở

1. **"Bản dịch tiếng Việt" cho chuỗi UI không làm được như yêu cầu.** Frontend không có lớp i18n, và FE-COPY-1 quy định chữ UI là tiếng Anh. Spec này cho bảng thuật ngữ một cột tiếng Việt chỉ dành cho tài liệu. Thêm i18n cho UI sẽ là một gói riêng. Người dùng có muốn điều đó không?
2. **Một unique index trên `lower(tags.name)`.** Nó sẽ khiến tên trùng không thể biểu diễn được (C10 chỉ kiểm trong handler). Cần quyết định với các bản cài đã có tên trùng: từ chối migration, hay đổi tên. Đổi tên âm thầm làm thay đổi rule nào chạm tới dữ liệu nào, vì engine khớp theo tên.
3. **Tên tab "Engine policies".** Được chọn để thay "Hand-written". Mục sidebar của khu vực là "Engine & policy", khá gần. "Policies written in the engine" là phương án dài hơn.

## 13. Ngoài mục tiêu

- **Các nhóm một mục của sidebar**: KI-225 trong `docs/superpowers/plans/2026-10-21-known-issues-fix-programme.md` (U3).
- **Kiểm chứng một mask theo từng người dùng** (N1): Gói 2, phác thảo ở §14.
- **Clearance StarRocks: NULL hay từ chối** (§11.2).
- **Kiểm id sai của resource-tags** (§6, §11.1).
- **Ghi chú SSO/localtest của demo stack** (E2).
- **Cập nhật `egret-data/docs`** (§15).

## 14. Phác thảo Gói 2: kiểm chứng một mask theo từng người dùng (N1)

Sẽ được viết sau thành spec riêng. Những gì nó phải giao:

1. **Dòng đầu tiên trả lời "chưa được thực thi"** trước mọi thứ khác. Khi không engine nào có profile phục vụ catalog, nói "Not enforced: <catalog> has no engine", và bỏ các dòng theo engine (hôm nay trạng thái này tồn tại dưới tên `no_profile_serves_the_catalog`, `routes.rs:526-537`).
2. **Một dòng tác dụng theo người dùng cho mỗi engine**: "member-sso reads email as **hashed** (rule *Hash personal data*)" / "as the real value (exempt by group data-team)" / "**refused** (clearance PII)". Row filter được xử lý tương tự: "sees rows where region = 'us' (rule X)".
3. **Mask trên Trino.** Hôm nay Explain chỉ đánh giá mask cho lần đọc cột trên StarRocks (`routes.rs:2347-2349`). Các mask policy của tag service trên Trino bị evaluator từ chối (`:723-726`). Gói 2 để evaluator đọc bản tải của tag service cho các tag đã phân giải của cột đích, và gọi `evaluate_data_mask` / `evaluate_row_filter` (evaluator đã cài đặt sẵn, theo `ExplainResult.tsx:1340-1356`), trên cả Trino lẫn StarRocks.
4. **Phác thảo API.** `ExplainProfile` có thêm:
   - `mask: Option<ExplainMask>` cho mọi engine (hôm nay chỉ có cho StarRocks);
   - `row_filter: Option<ExplainRowFilter { filter_expr, attribution }>`;
   - `effect_line: String`, dựng phía server để mọi bề mặt nói giống nhau.

   `POST /perm/explain/principal` đã tách theo từng thành viên, nên một nhóm có mỗi thành viên một dòng.
5. **UI.** Panel cột có thêm **"Check a person"** cạnh Who can read, hiện dòng tác dụng cho một người dùng. Phần preview của hộp thoại rule có thể đề nghị "Check with a person" sau khi Create.

## 15. Tài liệu

Quy tắc của repo (`CLAUDE.md`) yêu cầu mọi thay đổi người dùng nhìn thấy phải cập nhật `egret-data/docs` (mục Platform) kèm ảnh chụp màn hình mới. **Việc cập nhật đó được hoãn, chờ người dùng quyết định.** Gói này thay đổi:

- tên các tab, nút và mục (C1);
- dải Effect mới;
- gỡ tại chỗ;
- hộp thoại rule;
- tab Clearance;
- các thông báo lỗi của classification;
- một route API mới (B4) và các trường phản hồi mới (B3, B5).

Mỗi mục trên sẽ cần thay đổi tài liệu và ảnh chụp mới khi người dùng quyết định. Spec này và bản tiếng Việt của nó là tài liệu duy nhất mà gói này viết.
