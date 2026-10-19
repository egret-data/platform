# Thiết kế lại frontend Data Governance #4b: Classification — thiết kế

> Bản tham khảo tiếng Việt. Bản tiếng Anh `docs/en/specs/2026-10-19-dg-classification-design.md` là bản chuẩn; hai bản
> lệch nhau thì theo bản tiếng Anh.

Ngày: 2026-10-19. Trạng thái: thiết kế đã được người dùng duyệt ngày 2026-10-19 (C1, các phần 1–3 và phần kiểm thử, bên
dưới); các dòng ghi "default decision" là quyết định của tài liệu này. Q1–Q5 là các chỗ thiết kế đã duyệt
mâu thuẫn với code, tìm thấy khi đối chiếu tiền đề; chúng đã được quyết ngày 2026-10-19 (Q1 và Q5 do người dùng, Q2–Q4
là default decision). Base: main `309405b1` (#4a Engine & policy đã merge là #697).
Nhánh `feat/dg-classification`, worktree `egret-dgcl`.

Đây là sub-project #4b của spec chương trình `docs/en/specs/2026-10-16-dg-frontend-redesign-design.md` ("spec chương
trình"; quyết định F1–F12) và là nửa sau của #4 theo cách chia EP1 của #4a
(`docs/en/specs/2026-10-19-dg-engine-policy-design.md`). Phần 1 của spec chương trình giao cho nhóm sidebar
**Classification** "danh mục tag và clearance", thay Classifications và TagDetail; F12 thêm rằng trang đối tượng Data
liệt kê các policy viết tay chạm tới đối tượng, đánh dấu "hand-written", để chúng không bao giờ bị giấu. Mọi khẳng định
về code hiện có đều kèm `file:line` tại `309405b1`, đọc trong worktree này.

## Các tiền đề đã đối chiếu với code

Phần khảo sát trong brief đã được kiểm tra. Các câu sai hoặc thiếu được liệt kê trước; thiết kế đi theo code.

**Sai hoặc thiếu**

1. **Tag của catalog được khoá theo warehouse, không theo catalog, và chỉ catalog của egret mới gắn tag được.**
   `catalog_tag_assignments` có `warehouse_id uuid NOT NULL` và không có cột catalog
   (`migrations/0081_catalog_tag_targets.sql:9-17`); mọi chỗ đọc đều join `registry.catalogs c ON c.warehouse_id =
   a.warehouse_id` (`crates/egret-policy/src/store.rs:1342`, `:1759`). Chỉ catalog Iceberg của egret có `warehouse_id`
   (`migrations/0147_catalog_registry.sql:24-33`, một catalog cho mỗi warehouse `:37-38`), nên catalog bên ngoài không
   bao giờ mang tag (`crates/egret-controlplane/src/governance_tags.rs:15-19`). Vì vậy câu "catalog bên ngoài: chỉ đọc
   DB" của route C không có tác dụng với phần Data. `warehouse_id` **không có khoá ngoại**, nên một assignment có thể
   sống lâu hơn catalog của nó; route C báo các dòng đó thay vì bỏ đi (§4.4, CL11).
2. **Không cần migration.** Khoá chính của `catalog_tag_assignments` là `(tag_id, warehouse_id, schema_name,
   table_name, column_name)` (`0081:17`) và của `tag_assignments` là `(tag_id, object_type, object_id)`
   (`migrations/0080_tags.sql`); cả hai bắt đầu bằng `tag_id`, nên đọc theo tag là quét index. CL15 ghi quy tắc nếu
   điều này thay đổi.
3. **TagDetail có ba cảnh báo và một ghi chú giới hạn, không phải hai cảnh báo.** Group không được engine thực thi
   (`frontend/src/routes/governance/TagDetail.tsx:343-364`, hiện khi có group bắt buộc), chỉ chặn đọc trên cả hai engine
   (`:366-386`), phạm vi view trên Trino/StarRocks (`:387-401`), và giới hạn mở rộng 500 cột của StarRocks (`:402-408`),
   ba cái sau hiện khi có role bắt buộc. Cả bốn chuyển nguyên sang tab Clearance (§5.2).
4. **Một item Ranger nêu group hoặc role không khớp ai trên engine.** egret không chạy usersync, không viết
   `group-provider.properties` và không phục vụ roles download, nên item viết tay nêu group `g` hay role `r` không cho
   phép cũng không chặn ai; chỉ username trần và group `public` khớp
   (`crates/egret-perm-compile/src/members.rs:3-40`, đo được là D7; module doc của
   `crates/egret-perm-compile/src/clearance.rs`, D0: "a Ranger policy item naming a role matches nobody", và một item
   không khớp trong `denyExceptions` không miễn trừ ai). Compiler mở rộng principal group và role của chính egret thành
   username; policy viết tay không được biên dịch, nên không có gì mở rộng group hay role của nó. Cách đánh dấu "via
   group X" / "via role Y" đã duyệt cho route B sẽ mô tả một membership của egret mà engine không bao giờ áp dụng (Q1).
5. **Phân giải membership phía server đã có, trong `egret-perm-compile`, không phải `egret-perm`.** `read_user_via`
   (`crates/egret-perm-compile/src/people.rs:788`) trả về `StandsFor { kind, ref, via, on_engines, not_on_engines }`
   với `Via::{Itself, Group, Role, Workspace, Public, GroupRole, GroupWorkspace}` (`:294-314`, `:356-362`); các trang
   People đọc nó dưới tên `standsFor` (`frontend/src/routes/governance/people/tabs/PermissionsTab.tsx:169`,
   `people/reach.ts:33`). Nó mô tả cách principal của **grant egret** chạm tới một user. Route B dùng lại nó để lấy
   username, group và role của user.
6. **Khớp ANCESTOR không phải "applies".** `egret_ranger_eval::resource::MatchType` là `None | Self_ | Descendant |
   Ancestor | SelfAndAllDescendants` (`crates/egret-ranger-eval/src/resource.rs:47-67`), và với request tài nguyên
   Ranger chỉ đánh giá policy khi `SELF` hoặc `SELF_AND_ALL_DESCENDANTS` (`resource.rs:14-28`, đo live: một grant biên
   dịch thành `{catalog, schema}` bị từ chối select bảng). Policy viết trên schema với wildcard ở dưới (`ice.s.*.*`) là
   `SelfAndAllDescendants` với một bảng trong `s` và áp dụng; policy chỉ nêu `{catalog, schema}` là `Ancestor` và không
   áp dụng cho dữ liệu của bảng. Câu đã duyệt "catalog/schema cha tính là applies" chỉ đúng với dạng thứ nhất (Q2).
7. **Đường dẫn đã duyệt của route C trùng dạng với một route có sẵn.** `GET
   /api/control-plane/governance/tags/{catalog_id}` đã tồn tại, khoá theo id **catalog** (`governance_tags.rs:35`,
   `:62-64`). `…/governance/tags/{id}/usage` khoá theo id **tag** sẽ không đụng nhau trong axum (số segment khác) nhưng
   làm một tiền tố đường dẫn mang hai nghĩa id. Tiền lệ khoá theo tag là
   `GET /api/control-plane/classifications/{id}/view-exposure` (`crates/egret-controlplane/src/view_exposure.rs:209-211`)
   (Q3).
8. **Tab (5) "Hand-written tag policies" không có route nào được duyệt, và C2 cấm khớp ở client.** Tag policy nêu tag
   trong `resources.tag`, khớp bằng matcher của service-def tag (`wildCard: true`, `ignoreCase: false`,
   `crates/egret-policy/src/service-defs/tag.json:21`, `:53`, `:117`); làm việc đó trong trình duyệt là một matcher thứ
   hai (Q4).
9. **Tab Masks & filters của Data hôm nay không có ghi chú hand-written.** Ghi chú `EgretGrantsOnlyNote`
   (`frontend/src/routes/governance/tabParts.tsx:69-102`) được render ở Data `PermissionsTab.tsx:135`, Data
   `ColumnPanel.tsx:336`, People `PermissionsTab.tsx:176` và People `MasksFiltersTab.tsx:69`. Tab Masks & filters của
   Data nhận section mới mà không có ghi chú nào để thay.
10. **`enabled` là một cột, và `isEnabled` trong body có thể cũ.** Bản download chèn `isEnabled` từ
    `policy.authz_policies.enabled` (`store.rs:2187-2211`); lần kiểm live của #4a thấy `PUT /policies/{id}/enabled` để
    nguyên `policy_jsonb.isEnabled` (`docs/en/plans/2026-10-19-dg-engine-policy-live.md` §2.4). Route A, B và D đọc cột.
11. **409 khi xoá đến từ một khoá ngoại.** `perm.rules.tag_key REFERENCES tags(key)` không có `ON DELETE` từ chối xoá
    khi một rule chọn **key** của tag (`crates/egret-bi/src/tags.rs:322-333`, `:364-381`). Rule viết trên tag khác có tên
    trùng (`TagRulesSection.tsx` `reachText`) không chặn. Lệnh xoá cũng cascade cả hai bảng assignment và
    `classification_policies` rồi biên dịch lại trong cùng transaction (`tags.rs:309-388`); 204 của nó không mang báo
    cáo skip.
12. **Hai query key cho một danh sách.** Các trang đọc tag qua hook `apiJson` viết tay, key `["tags"]`
    (`frontend/src/api/tags.ts:41-95`); orval cũng sinh `useControlPlaneTagsListTags`
    (`frontend/src/api/generated/control-plane-tags/control-plane-tags.ts:367`) với key riêng. Trang đọc một key mà
    mutation invalidate key kia chính là lỗi D4 của #4a (danh sách cũ tới khi tải lại). CL16 chốt một key.
13. **Các trang classification không lazy.** `Classifications` và `TagDetail` được import tĩnh (`frontend/src/App.tsx:199-200`)
    và route ở `:652-653`; #4b biến chúng thành một khu lazy có bundle guard (FE-ROUTE-2).
14. **Engine khớp theo tên tag; rule và key không phải tên; tên không duy nhất.** `tags.key` là `UNIQUE`, `tags.name`
    thì không (`0080`); bản tag download điền type của tag từ `tags.name` và explain phân giải `tags.name`
    (`store.rs:1671-1677`, `:1214`), trong khi `perm.rules` chọn `tags.key`. Hai tag cùng tên chia nhau mọi tag policy
    của engine trên tên đó. Tab (5) nói ra khi điều này xảy ra (CL10).
15. **Policy viết tay có thể mang cấu trúc egret không đánh giá.** `RangerPolicy.unmodelled`
    (`crates/egret-ranger-eval/src/model.rs:154-158`, các biến thể `:403-…`: conditions, validity schedules, zones) và
    `isDenyAllElse` (`model.rs:145`). Việc khớp tài nguyên vẫn chính xác; các dòng hiển thị những cấu trúc này để
    "applies" không bị đọc thành "quyết định" (CL9).
16. **Hàm dựng request của explain là private.** `request_resource(engine, catalog_name, target)`
    (`crates/egret-perm-compile/src/routes.rs:2957-2986`) dựng request bằng `registry.catalogs.name` lấy từ
    `catalogs_for_service_via` (`routes.rs:2270-2280`, `store.rs:1363-1375`) và tên cấp của `resource_level_names`
    (`crates/egret-perm-compile/src/grants.rs:218-223`; cấp hai của StarRocks là `database`). Route A phải dựng đúng
    request đó, nên hàm được chuyển sang một module dùng chung thay vì chép (CL7).
17. **Một lần kiểm quyền đọc BI là một lần gọi PDP cộng hai lần đọc cho mỗi đối tượng.** `Pdp::authorize` nhận một
    subject, action và resource (`crates/egret-core/src/authz.rs:70`, `:108-124`); không có batch. `object_ref` đọc folder
    và workspace của đối tượng (`crates/egret-bi/src/shares.rs:116-135`) và gate của folder đọc `entity_type`
    (`shares.rs:81-98`); `read_action` là private (`shares.rs:58-67`). Không cần `WorkspaceContext`: PDP được hỏi về
    subject của principal và đối tượng. Chi phí mỗi đối tượng được đo ở Task 1 (M4).
18. **Số dòng trong khảo sát.** `usable_tag_services_via` ở `store.rs:1407` (không phải 1404);
    `resolved_tags_for_resource_via` ở `store.rs:1696` (không phải ~1666).

**Đúng như đã nêu** (đã kiểm): route `App.tsx:652-653`; `Classifications.tsx` 443 dòng, `TagDetail.tsx` 630; API tag trong
`crates/egret-controlplane/src/tags.rs` (`GET` `tag:read` `:111`; `POST`/`PATCH`/`DELETE` `tag:manage` `:156`/`:191`/`:222`;
access-policy `PUT`/`DELETE` `:291`/`:334` trả `{ok, skipped[]}`); `view_exposure.rs:211`; bảng 0080
`tags`/`tag_assignments`, 0081 `catalog_tag_assignments` (`column_name = ''` là bảng; chỉ bảng và cột, 0177), 0082/0083
`classification_policies`; clearance chỉ biên dịch thành deny và cổng chỉ-group biên dịch thành không gì, báo skipped
(`clearance.rs`); `covers` không phân biệt hoa thường (`crates/egret-perm/src/eval.rs:303-352`);
`services_for_catalog_via` `store.rs:1471`; `PolicySummary` `crates/egret-policy/src/admin/mod.rs:276-345` (`resource_path`
nối giá trị đầu của mỗi cấp); hand-written = `managed_by = 'operator'` (tiền đề 1 của #4a); Cedar chỉ có
`tag:read`/`tag:manage` cho tag (`crates/egret-core/src/rbac.rs:276-287`); `data_steward` giữ cả hai (seed 0080);
`TagDto.assignment_count` chỉ đếm đối tượng BI (`crates/egret-bi/src/tags.rs:36-38`).

## Quyết định

| id | quyết định | bởi |
|---|---|---|
| C1 | Trang tag có tab **Where used** bao cả Data và BI | user, 2026-10-19 |
| C2 | Phạm vi của policy viết tay được tính bằng matcher của `egret-ranger-eval` (`match_policy_resources` / `PreparedPolicyResources`, `resource.rs:147-262`), không bao giờ bằng matcher tự viết | default decision (2026-10-19) |
| CL1 | Giữ nhóm sidebar **Classification**, gate `tag:read`. Index `/governance/classifications`: bảng tag (tên + màu, key, mô tả, số bảng/cột mang nó, số đối tượng BI, clearance có/không kèm số role và group), dải tóm tắt (tổng, có clearance, chưa dùng), tạo mới (tên → key suy ra; `tag:manage` hoặc platform admin) | user, 2026-10-19 (phần 1) |
| CL2 | `/governance/classifications/:id`: header (tên, màu, key, mô tả, sửa tại chỗ) và các tab **Where used · Clearance · Mask & filter · View exposure · Hand-written**; hộp xác nhận xoá nêu hệ quả và yêu cầu gõ key; giữ cách xử lý 409; URL không đổi; mọi link hiện có vẫn chạy | user, 2026-10-19 (phần 1) |
| CL3 | Route A `GET /api/control-plane/governance/objects/{catalog_id}/handwritten?schema=&table=&column=` (`policy:read`) và route B `GET /api/control-plane/governance/principals/{kind}/{id}/handwritten` (`policy:read`); Data và People có section "Hand-written policies (N)"; thiếu `policy:read` thì có lời giải thích | user, 2026-10-19 (phần 2) |
| CL4 | Route C (mức dùng của tag, `tag:read` HOẶC `policy:read`) với trần 500 lần kiểm và nói rõ phần chưa kiểm; `GET /tags` thêm số đếm Data theo kiểu cộng thêm; không migration (tiền đề 2) | user, 2026-10-19 (phần 3) |
| CL5 | Kiểm thử như §9: test DB cho mỗi route, test vi sai route A với `POST /perm/explain`, vitest đỏ trước, bundle guard `test:bundle:governance-classification`, kiểm live với Trino và StarRocks thật, dự đoán trước | user, 2026-10-19 |
| CL6 | Ngoài phạm vi: quyền gắn tag (Data `catalog_registry:update`, BI theo từng đối tượng) giữ nguyên; độ lệch rule→`tags.key` và engine→`tags.name` ghi lại làm follow-up | user, 2026-10-19 |
| CL7 | **Một quy tắc khớp, trong một module.** `crates/egret-perm-compile/src/handwritten.rs` sở hữu `object_request` (chuyển từ `request_resource` trong `routes.rs`, explain gọi lại nó), `object_match` (các `MatchType` của một policy → `applies` / `part` / `parent` / `none`, §4.1) và `item_summary`. Route A, B, D gọi nó; không file nào khác quyết định việc khớp | default decision (2026-10-19) |
| CL8 | Handler route nằm ở `crates/egret-controlplane/src/governance_handwritten.rs` (A, B, D) và `governance_tag_usage.rs` (C), tag `control-plane-governance`; phần DB ở `egret-perm-compile` (A, B, D) và `egret-bi` (C) | default decision (2026-10-19) |
| CL9 | Một dòng hiển thị `isDenyAllElse` và mọi cấu trúc `unmodelled` thành câu ("Has conditions egret does not evaluate"); vẫn báo kết quả khớp, và dòng không bao giờ khẳng định policy là thứ quyết định | default decision (2026-10-19) |
| CL10 | Tab (5) nói khi có tag khác cùng tên: "Tag K2 has this name too; engine tag policies on this name apply to both." | default decision (2026-10-19) |
| CL11 | Dòng Data của route C có warehouse không catalog nào nêu được đếm: "N assignments are in a warehouse no catalog names." Dòng BI có đối tượng không còn tồn tại được đếm: "N assignments point at deleted objects." Không dòng nào bị bỏ âm thầm | default decision (2026-10-19) |
| CL12 | Route C kiểm đối tượng BI theo thứ tự cố định (workspace id, kind, object id) tối đa 500 mỗi lần gọi; phần còn lại là `{remaining: N}` và tab nói "Not all BI objects were checked: N remain.", không có "show more" trong #4b | default decision (2026-10-19) |
| CL13 | Saved query và folder không có trang chi tiết: cái đọc được hiển thị tên và loại, không link; dataset, chart, dashboard và app link tới `/explore/<workspace slug>/<kind path>/<id>` (Task 1 M6 xác nhận) | default decision (2026-10-19) |
| CL14 | Số rule trong hộp xoá là các rule có selector key bằng key của tag này (các chặn khoá ngoại, tiền đề 11); thiếu `policy:read` thì ghi "Rules: needs policy:read to count". Khi số rule > 0, nói lệnh xoá sẽ bị từ chối và vẫn giữ nút (server quyết; toast 409 giữ nguyên) | default decision (2026-10-19) |
| CL15 | Không migration. Nếu một thay đổi sau cần, số của nó được chọn bằng `ls migrations \| tail -3` ngay trước khi merge (các PR workflow song song lấy số) | user, 2026-10-19 |
| CL16 | Một query key cho mỗi danh sách: các trang giữ `useTags` (`["tags"]`); kiểu `Tag` có thêm trường mới; mọi mutation tag (hook cũ và hộp thoại mới) invalidate `["tags"]` **và** key của route C cho tag đó. Route mới A–D dùng hook orval (FE-DATA-2) | default decision (2026-10-19) |
| CL17 | Tab của trang tag nằm trong `?tab=` (`where-used` mặc định, `clearance`, `rules`, `views`, `handwritten`); link mang nghĩa một tab thì truyền nó: nguồn clearance của Engine & policy và Clearances của People → `?tab=clearance`; RulePage của People → `?tab=rules` | default decision (2026-10-19) |
| Q1 | "via group X / via role Y" của route B (tiền đề 4): liệt kê các item đó với dấu "Names group X, which this user is in. Engines resolve no groups, so on the engine it reaches nobody." (role tương tự), trong một nhóm riêng dưới nhóm trực tiếp; phần tóm tắt item của route A đánh dấu mọi principal group và role như vậy; `public` và `{USER}` là "applies to everyone" | user, 2026-10-19 |
| Q2 | Các mức khớp (tiền đề 6): `Self_` hoặc `SelfAndAllDescendants` → **applies to this object** (kèm "through a pattern" khi một cấp khớp bằng wildcard, exclude hoặc recursive); `Descendant` → **applies to part of it**; `Ancestor` → liệt kê là **set on a parent** với "Names <catalog[.schema]> only; engines do not apply it to this object's data." Với tag policy: tag trên đối tượng hoặc tổ tiên → applies; trên vài cột bên dưới → part | default decision (2026-10-19) |
| Q3 | Đường dẫn route C (tiền đề 7): `GET /api/control-plane/classifications/{id}/usage`, cạnh `…/{id}/view-exposure` | default decision (2026-10-19) |
| Q4 | Dữ liệu của tab (5) (tiền đề 8): route D `GET /api/control-plane/classifications/{id}/handwritten` (`policy:read`): mọi policy `operator` của mọi tag service dùng được mà tài nguyên tag khớp tên tag này qua matcher của service-def tag, kèm các profile và engine tải nó | default decision (2026-10-19) |
| Q5 | Đối tượng BI không đọc được (tiền đề 17, §4.4): **chỉ đếm**, "N objects you cannot read"; không đếm workspace, không nêu tên workspace. Route C chỉ trả số đối tượng cho phần không đọc được | user, 2026-10-19 |

## 1. Những gì đang có

- **Trang.** `Classifications.tsx`: `FilterableTable` (Tag, Key, Description, BI objects, badge Gated), hộp tạo với
  `slugifyTagKey`, menu dòng có xoá; gate `platform_admin` hoặc `tag:manage`. `TagDetail.tsx`: header (ô màu, tên, key,
  badge Gated, số BI), form Details tự lưu khi blur, Clearance (`ClearanceTable`: một bảng role và group bắt buộc, ghi
  cả bộ mỗi lần thêm/bớt qua `PUT`/`DELETE …/access-policy`; `skipped` không rỗng thì hiện "Saved, but not compiled into
  any engine: …", `:131-164`), `ViewExposureSection`, `TagRulesSection` (`policy:read`), xoá với `confirmText="delete"`
  (`:428-446`) và `toastTagDeleteFailure` (`tagDeleteFailure.ts`).
- **Route.** `GET/POST /control-plane/tags`, `PATCH/DELETE /control-plane/tags/{id}`,
  `PUT/DELETE /control-plane/tags/{id}/access-policy` (`tags.rs`); `GET /control-plane/classifications/{id}/view-exposure`;
  tag đối tượng BI `GET/PUT/DELETE /ws/{ws}/objects/{kind}/{id}/tags[/{tag_id}]` (`shares.rs:544-666`); tag đối tượng
  Data `GET /governance/tags/{catalog_id}?schema=&table=` (`governance_tags.rs`); danh sách rule `GET /perm/rules`.
- **Link vào.** `overview/refUrl.ts:44,46`, `overview/AttentionSection.tsx:111`, `engines/tabs/PoliciesTab.tsx:64`,
  `people/tabs/ClearancesTab.tsx:117`, `people/RulePage.tsx:154`; `/control-plane/classifications/*` →
  `LegacyGovernanceRedirect` (`App.tsx:687`).
- **Policy viết tay.** Các dòng `policy.authz_policies` có `managed_by = 'operator'`; profile chính và tag service của
  nó (`usable_tag_services_via`, `store.rs:1407`); các profile của một catalog (`services_for_catalog_via`, `:1471`); tab
  Advanced của trang engine và URL editor (`frontend/src/routes/governance/engines/engineUrl.ts:39-55`).

## 2. Kiến trúc thông tin

| URL | trang | guard |
|---|---|---|
| `/governance/classifications` | index | `tag:read` |
| `/governance/classifications/:id?tab=` | trang tag; `tab` ∈ `where-used` (mặc định), `clearance`, `rules`, `views`, `handwritten` | `tag:read` (tab `rules` và `handwritten` cần `policy:read`, nếu không thì có lời giải thích) |

Cả hai nằm trong khu lazy `frontend/src/routes/governance/classifications/` (`ClassificationsRoutes.tsx`) mount ở
`classifications/*` trong `LazyRouteArea` dưới `RequirePermission perm="tag:read"`, với bundle guard
`npm run test:bundle:governance-classification` (`frontend/scripts/assert-governance-classification-lazy.mjs`), ghi trong
FE-ROUTE-2, FE-TEST-4 và `frontend/CLAUDE.md`. Sidebar không đổi (`Sidebar.tsx:113-114`). `Classifications.tsx` và
`TagDetail.tsx` bị xoá khi hành vi của chúng đã chuyển đi (test được chuyển theo, không bỏ).

## 3. Index (`/governance/classifications`)

`ListPageShell` (FE-PAGE-1): `h1` "Classifications", toolbar **New classification** (`tag:manage` hoặc platform admin),
`SummaryStrip`: "N classifications · N with clearance · N unused" (unused = không có assignment Data và BI nào; số nhiều
theo FE-COPY-4). Cột của `FilterableTable`:

| cột | nguồn |
|---|---|
| Name (ô màu + link) | `TagDto.name`, `.color` |
| Key | `.key`, mono |
| Description | `.description` hoặc "—" |
| Data | "T tables · C columns" từ `TagDto.catalog_tables`, `.catalog_columns` (mới, §4.4); "—" khi cả hai bằng 0 |
| BI objects | `.assignment_count` |
| Clearance | "None", hoặc "R roles · G groups" kèm biểu tượng khoá và chữ (FE-A11Y-4) |

Hộp tạo (FE-PAGE-3, một đến ba trường): Name, Key (suy từ tên bằng `slugifyTagKey` cho tới khi bị sửa, sau đó giữ),
Description, Colour. 409 (key trùng) hiện thông điệp của server cạnh Key, hộp vẫn mở (FE-ACTION-3). Thành công thì chuyển
tới trang tag mới (không toast, FE-ACTION-2). Menu dòng giữ Delete, qua cùng hộp thoại của trang tag (§5.6). Rỗng: "No
classification yet." kèm New classification khi được phép; lọc không trúng thì nói vậy kèm Clear.

## 4. Backend

### 4.1 Quy tắc khớp (CL7, Q2)

`crates/egret-perm-compile/src/handwritten.rs`:

```rust
pub enum ObjectLevel { Catalog, Schema, Table, Column }
pub struct ObjectTarget { pub catalog_id: Uuid, pub schema: Option<String>, pub table: Option<String>, pub column: Option<String> }
pub fn object_request(engine: EngineType, catalog_name: &str, target: &ObjectTarget) -> RequestResource; // request_resource của explain, chuyển sang
pub enum ObjectMatch { Applies { through_pattern: bool }, Part, Parent, None }
pub fn object_match(prepared: &PreparedPolicyResources, policy: &RangerPolicy, request: &RequestResource) -> ObjectMatch;
pub struct ItemSummary { pub bucket: Bucket, pub users: Vec<String>, pub groups: Vec<PrincipalMark>, pub roles: Vec<PrincipalMark>,
                         pub everyone: bool, pub accesses: Vec<String>, pub mask_type: Option<String>, pub row_filter: Option<String> }
pub fn item_summary(policy: &RangerPolicy) -> Vec<ItemSummary>;
```

- `object_match` lấy kết quả tốt nhất trên mọi resource map (`ResourceMapSource::Primary` và từng `Additional(i)`):
  `Self_`/`SelfAndAllDescendants` > `Descendant` > `Ancestor` > `None`, ánh xạ như Q2. `through_pattern` đúng khi giá trị
  policy ở một cấp mà request nêu không phải đúng giá trị literal đó (so sau khi fold), tức khớp bằng `*`/`?`, exclude
  hoặc `isRecursive`.
- `explain` gọi `object_request` thay vì bản sao private của nó (không đổi hành vi; test của nó vẫn xanh, không sửa).
- `Bucket` ∈ `allow`, `deny`, `allow_exception`, `deny_exception`, `mask`, `row_filter`. `everyone` đúng với group
  `public` hoặc user `{USER}`; `PrincipalMark` mang tên và `on_engines: false` cho mọi group trừ `public` và mọi role
  (tiền đề 4).

### 4.2 Route A: policy viết tay chạm tới một đối tượng Data (CL3)

`GET /api/control-plane/governance/objects/{catalog_id}/handwritten?schema=&table=&column=`, `policy:read` (không thì
403). 400 khi có `table` mà thiếu `schema`, có `column` mà thiếu `table`, hoặc giá trị rỗng (quy tắc của browse). 404 với
catalog không biết. Một snapshot `REPEATABLE READ, READ ONLY`.

1. Profile: `services_for_catalog_via(catalog_id)`, giữ những cái `engine_type_for` nhận (Trino, StarRocks); tag
   service của chúng: `usable_tag_services_via`. Engine của mỗi profile từ `engine_mgmt.engines` (`id`, `name`, `type`,
   `origin`). Engine bên ngoài chỉ đọc từ DB; không gọi tới gì.
2. Không xác định được → `{ state: "not_determined", reason }`, không bao giờ là danh sách rỗng: `no_engine_attached`
   (không attachment), `no_profile` (engine được gắn không bind profile nào), `catalog_not_in_profile`
   (`catalogs_for_service_via` của mọi profile thiếu catalog).
3. Resource policy: mọi dòng của mỗi profile có `managed_by = 'operator'` (gồm cả dòng tắt, `enabled` lấy từ cột, tiền
   đề 10), parse qua `ServicePolicySet` với service-def theo loại profile (`get_service_def_via`); request =
   `object_request(engine, tên catalog từ catalogs_for_service_via, target)`; `object_match` ≠ `None` → một dòng.
4. Tag policy (profile Trino có tag service): tag của đối tượng — với bảng hoặc cột, `resolved_tags_for_resource_via`
   (tag của bảng áp cho các cột của nó; ancestor), cộng với bảng, schema hoặc catalog thì các tag trên thứ nằm dưới nó
   (`tag_assignment_coordinates_via` / các dòng assignment dưới target) → "part". Mỗi tag policy của tag service (chỉ
   `operator`) có tài nguyên tag khớp một tên tag qua `PreparedPolicyResources` của service-def tag (request
   `{tag: <name>}`) → một dòng với `viaTag {name, on: "this" | "parent" | "below", column?}`; `this`/`parent` → applies,
   `below` → part.
5. Seeded: các dòng `managed_by = 'infra'` của cùng profile mà khớp → đếm theo profile, không liệt kê.
6. Dòng có body không parse được vẫn được liệt kê với `match: "unknown"` và lỗi parse; không bao giờ bị bỏ.

```text
{ catalog: { id, name },
  target: { schema?, table?, column? },
  state: "listed" | "not_determined",
  reason: "no_engine_attached" | "no_profile" | "catalog_not_in_profile" | null,
  profiles: [ { serviceId, name, type, tagService: string|null,
                engines: [ { id, name, type, origin } ] } ],
  policies: [ { id, name, service, inTagService: bool, policyType: "access"|"masking"|"rowfilter",
                match: "applies"|"part"|"parent"|"unknown", throughPattern: bool,
                viaTag: { name, on: "this"|"parent"|"below", column: string|null } | null,
                resourceSummary: string, enabled: bool, denyAllElse: bool, unmodelled: [string],
                items: [ ItemSummary ], parseError: string|null } ],
  seeded: [ { service, count } ] }
```

Thứ tự dòng: `applies` trước `part` trước `parent` trước `unknown`, rồi tên profile, rồi tên policy.

### 4.3 Route B: policy viết tay nêu một principal (CL3, Q1)

`GET /api/control-plane/governance/principals/{kind}/{id}/handwritten`, `policy:read`. `kind` ∈ `user` (`id` = uuid của
user), `group`, `role` (`id` = tên); khác thì 400; principal không biết thì 404 (quy tắc của các route People).

- Danh tính: `users.username` của user và, từ `read_user_via`, các group và role nó giữ (`Via::Group`, `Via::Role`;
  không lấy `GroupRole`/`GroupWorkspace`, vốn hai lần chỉ có trong egret). Group: tên của nó. Role: tên của nó.
- Mọi policy `operator` của mọi profile và tag service có item nêu danh tính đó: `users` chứa username → `direct`;
  `groups` chứa một group của user (hoặc chính group đó trên trang group) → `group` (dấu của Q1); `roles` tương tự →
  `role`; `public`/`{USER}` → gom riêng thành `everyone`, không bao giờ quy cho user.
- User không có username: `{ state: "no_username" }` chỉ kèm số đếm `everyone`, vì không item nào nêu được họ trên engine
  (`NotOnEngines::NoUsername`, `people.rs:328-330`).

```text
{ principal: { kind, id, label, username: string|null },
  state: "listed" | "no_username",
  policies: [ { id, name, service, inTagService, policyType, resourceSummary, enabled, denyAllElse, unmodelled,
                naming: [ { bucket, how: "direct"|"group"|"role", via: string|null, onEngines: bool } ],
                engines: [ { id, name } ] } ],
  everyone: { count, policies: [ { id, name, service, bucket } ] } }
```

### 4.4 Route C và số đếm của `/tags` (CL4, Q3)

`GET /api/control-plane/classifications/{id}/usage` (Q3), gate `tag:read` HOẶC
`policy:read` (`require_governance_read`, `governance_browse.rs:337-348`). 404 với tag không biết.

- **Data**: `catalog_tag_assignments` theo `tag_id`, `LEFT` join `registry.catalogs` trên `warehouse_id`. Nhóm theo
  catalog rồi schema rồi bảng: `{ catalogId, catalogName, schema, table, onTable: bool, columns: [string] }`; dòng có
  warehouse không catalog nào nêu → đếm `orphaned` (CL11).
- **BI**: `tag_assignments` theo `tag_id`, sắp `(workspace_id, object_type, object_id)` sau khi phân giải workspace của
  từng đối tượng; với 500 cái đầu: `object_ref` và kind của gate, rồi `Pdp::authorize(subject, read_action, object)`.
  Đọc được → `{ kind, id, name, workspaceSlug }`; không đọc được → chỉ đếm, không kèm số hay tên workspace (Q5); đối tượng mất →
  đếm `deleted` (CL11); quá 500 → `remaining` (CL12). `egret-bi` thêm `pub fn read_action_for_gate` (chính `read_action`
  private) và `pub async fn object_name` để route không phải chép chúng.

```text
{ tag: { id, key, name },
  data: { tables: [ { catalogId, catalogName, schema, table, onTable, columns: [string] } ],
          tableCount, columnCount, orphaned },
  bi: { readable: [ { kind, id, name, workspaceSlug } ],
        unreadable: { count }, deleted, checked, remaining } }
```

**`GET /control-plane/tags`** thêm hai trường cho mỗi `TagDto`, theo kiểu cộng thêm và snake_case như DTO
(`tags.rs:38-61`): `catalog_tables` (các `(warehouse, schema, table)` khác nhau có dòng cấp bảng) và `catalog_columns`
(các dòng có `column_name <> ''`), một truy vấn group by, join trong bộ nhớ như phần phủ policy (`tags.rs:113-133`).
`assignment_count` không đổi. Response của create/update mang số 0 (chúng vốn để trống clearance, `tags.rs:52-53`).

### 4.5 Route D: tag policy viết tay của một classification (Q4)

`GET /api/control-plane/classifications/{id}/handwritten`, `policy:read`. Mọi profile Trino có tag service dùng được;
các dòng `operator` của mỗi tag service có tài nguyên tag khớp **tên** của tag này (matcher của service-def tag, request
`{tag: name}`); mỗi dòng kèm các profile (và engine) có tag service đó, `items`, `enabled`, `unmodelled`; cộng
`sameName: [ { id, key } ]` cho các tag khác cùng tên (CL10). Không có tag service nào → `{ state: "no_tag_service" }`
("No engine profile has a tag service, so no engine downloads tag policies.").

## 5. Trang tag (`/governance/classifications/:id`)

**Header.** Ô màu, tên (`h1`), key (badge mono), mô tả; badge "Clearance" kèm khoá khi bị gate. Với `tag:manage` hoặc
platform admin: **Edit** mở form tại chỗ (tên, màu, mô tả; Save/Cancel, không tự lưu khi blur; `useUnsavedChangesWarning`,
FE-ACTION-5) — 4xx giữ form kèm thông điệp của server; và **Delete** (§5.6). Id không biết → "No classification with this
id" kèm link về index (không phải lỗi chung). Request tag lỗi → lỗi kèm Retry; không bao giờ là "not found".

**Tab** (`ui/tabs`, `?tab=`, FE-PAGE-2): Where used · Clearance · Mask & filter · View exposure · Hand-written. Mỗi tab có
skeleton, lỗi (thông điệp server, Retry) và trạng thái rỗng riêng (FE-STATE-1–3); một request lỗi không bao giờ làm trắng
tab khác.

### 5.1 Where used (C1)

Từ route C. Hai section.
- **Data**: nhóm theo catalog → schema; mỗi dòng bảng: "Table" khi `onTable`, và "Columns: a, b" khi có cột gắn tag; link
  tới `/governance/data/<catalog>/<schema>/<table>` (và `?column=` cho từng cột). `orphaned` → câu của nó. Rỗng: "No
  table or column carries this classification."
- **BI**: các đối tượng đọc được (biểu tượng và chữ của kind, tên, workspace, link theo CL13); rồi "N objects you cannot read." (Q5) khi `unreadable.count > 0`; câu `deleted`; `remaining > 0` → "Not all BI objects
  were checked: N remain." Rỗng: "No BI object carries this classification."
- Request lỗi hiện lỗi kèm Retry, không bao giờ hiện các câu rỗng (bài học #4a: tải lỗi không phải là một sự thật).

### 5.2 Clearance

Trình sửa clearance hiện tại, hành vi giữ nguyên: một bảng role và group bắt buộc, thêm bằng loại + picker có tìm kiếm,
bớt từng dòng; ghi `PUT`/`DELETE …/access-policy`; kết quả `skipped` hiện trong tab ("Saved, but not compiled into any
engine: …"). Bốn câu của tiền đề 3, mỗi câu dưới điều kiện hiện có của nó. Một dòng mới dưới bảng, hiện khi có role bắt
buộc: "Compiles to one deny tag policy in each Trino profile's tag service, and on StarRocks to a select deny per tagged
column." Focus của picker trở về nút mở khi đóng (#4a D3).

### 5.3 Mask & filter

`TagRulesSection` như hôm nay, mỗi rule link tới trang rule (`/governance/rules/:id`). Thiếu `policy:read`: "Rules are
read with the policy:read permission, which this account does not have."

### 5.4 View exposure

`ViewExposureSection` như hôm nay (`tag:read`).

### 5.5 Hand-written (Q4)

Từ route D. Dòng: tên policy → URL editor của engine (`editorUrl({engineId}, kind, id)` kèm `?scope=tag`, hoặc của trang
profile với profile chưa bind), engine, loại, item (principal kèm dấu Q1, access / loại mask / filter), enabled, các câu
`unmodelled`; câu `sameName` (CL10). Thiếu `policy:read`: lời giải thích. Rỗng: "No hand-written tag policy names this
classification."

### 5.6 Xoá (CL2, CL14)

`ConfirmDeleteDialog` với `confirmText` = key của tag. Nội dung, từ route C và danh sách rule: "Deleting K removes it from
T tables and C columns, B BI objects (and U you cannot read), and removes its clearance (R roles, G groups). N rules select
it: the delete will be refused until they are deleted or retargeted." Phần không biết thì nói lý do (thiếu `policy:read`
→ dòng rule của CL14; route C lỗi → "Usage could not be read: <message>"). Thành công: chuyển về index (không toast).
409: `toastTagDeleteFailure` như hôm nay, đóng hộp, focus trở lại nút Delete.

## 6. Hand-written trên Data và People (CL3)

Một `HandWrittenSection` dùng chung (`frontend/src/routes/governance/handwritten/HandWrittenSection.tsx`, nằm trong chunk
của Data và People qua import của chính chúng): tiêu đề "Hand-written policies (N)", mỗi dòng có badge "Hand-written",
chữ mức khớp ("Applies to this object", "Applies to part of it", "Set on a parent", "Could not be matched"), "through a
pattern" khi có, tóm tắt item, enabled, và link tới editor Advanced của engine. Số seeded: "N egret-seeded policies also
reach this object; see Engine & policy." kèm link tới tab Policies của từng trang engine. `not_determined` → câu lý do.
Thiếu `policy:read` → "Hand-written policies are read with the policy:read permission, which this account does not
have." Lỗi → lỗi kèm Retry, không bao giờ là "None".

- **Data**: `PermissionsTab` (dòng access), `MasksFiltersTab` (dòng masking và row-filter) và `ColumnPanel` (tất cả, cho
  cột đó) hiện section dưới grant/rule của egret, thay `EgretGrantsOnlyNote` bằng: "egret grants are above; hand-written
  policies below. To see who can actually read, use Who can read?" (link như hôm nay).
- **People**: `PermissionsTab` (access) và `MasksFiltersTab` (masking, row filter) từ route B, cùng section, dòng nhóm
  theo Direct · Through a group or role (Q1) · Applies to everyone (số đếm, mở rộng được).

## 7. Link

URL không đổi, nên mọi link vẫn chạy; link mang nghĩa một tab thêm `?tab=` (CL17): `engines/tabs/PoliciesTab.tsx:64` và
`people/tabs/ClearancesTab.tsx:117` → `?tab=clearance`; `people/RulePage.tsx:154` → `?tab=rules`. `overview/refUrl.ts:44,46`
và `AttentionSection.tsx:111` không đổi.

## 8. Lỗi và trạng thái

Như #4a: mỗi tab và section tự lo trạng thái; lỗi không bao giờ hiển thị thành rỗng; câu rỗng nêu sự thật; control
thiếu quyền bị ẩn hoặc nói lý do (FE-ACTION-4); lệnh ghi invalidate đúng các key nó đổi (CL16) và mọi 2xx là thành công
(#4a D2: create trả 201); hộp thoại trả focus về nút mở (#4a D3).

## 9. Kiểm thử (CL5)

- **Test DB backend** (`egret_test_db::url()`, fixture giới hạn trong id của chính nó, `CedarPdp` thật):
  - `handwritten.rs` (thuần): `object_match` theo từng `MatchType` và nguồn map, `through_pattern` cho `*`, `?`, exclude,
    recursive và một literal khác hoa thường (không phải pattern); `item_summary` theo từng bucket, `public`/`{USER}` →
    `everyone`, group và role `on_engines: false`.
  - Route A: schema wildcard → applies (through a pattern); exclude loại bảng → none; recursive; hoa thường;
    `{catalog, schema=s, table=*, column=*}` → applies cho `s.t`; chỉ `{catalog, schema}` → parent (Q2); policy trên một
    cột → part ở bảng và applies ở cột đó; tag policy qua tag của bảng → applies ở bảng và ở cột của nó; qua tag của cột
    → part ở bảng; dòng seeded chỉ được đếm; dòng tắt được liệt kê với `enabled: false` dù body ghi `isEnabled: true`;
    body không parse được → `unknown`; catalog không có engine → `no_engine_attached`; engine không có profile →
    `no_profile`; profile của engine bên ngoài được liệt kê, không gọi tới gì; 403 khi thiếu `policy:read`; các 400;
    404.
  - **Vi sai**: với fixture chỉ có policy viết tay (allow, deny, mask) trên profile Trino và StarRocks, và với mỗi cặp
    (đối tượng, user) của một lưới, policy viết tay quyết định của `POST /perm/explain` (nếu có) nằm trong tập `applies`
    của route A cho đối tượng đó; và mọi policy allow `applies` đang bật có item nêu user hoặc là thứ explain chọn quyết
    định, hoặc bị một policy quyết định mà explain nêu vượt hạng. Fixture có cấu trúc unmodelled bị loại và được liệt kê.
  - Route B: trực tiếp; qua group và qua role đánh dấu theo Q1; `public` và `{USER}` → chỉ `everyone`; user không
    username → `no_username`; trang group và role; 403; 404.
  - Route C: đối tượng BI đọc được và không đọc được với user thiếu workspace; đối tượng BI bị gate không đọc được khi
    thiếu clearance; trần (501 đối tượng → `checked: 500, remaining: 1`); đối tượng đã xoá → `deleted`; warehouse mồ côi
    → `orphaned`; 403 khi thiếu cả hai quyền, 200 khi chỉ có `tag:read` và khi chỉ có `policy:read`; số đếm của `/tags`
    (dòng cấp bảng và dòng cột; tag không liên quan bằng 0), các test route tag hiện có xanh và không sửa.
  - Route D: khớp tên, giá trị tag wildcard, hoa thường (không fold: `ignoreCase: false`), `sameName`, không có tag
    service.
- **Vitest frontend**, đỏ trước theo từng task: route và khu lazy; cột index, dải tóm tắt, tạo mới (key suy ra, 409 cạnh
  Key), lọc không trúng; sửa header trang tag (lưu, huỷ, giữ 4xx, cảnh báo chưa lưu), not-found khác lỗi; loading/rỗng/lỗi
  của từng tab, `?tab=`; Where used đọc được/không đọc được/remaining/orphaned/deleted; ghi Clearance và `skipped`, bốn câu
  và dòng biên dịch; Mask & filter và Hand-written khi thiếu `policy:read`; hệ quả trong hộp xoá, gõ key, dòng rule, 409;
  `HandWrittenSection` theo từng mức khớp, badge, link, dòng seeded, `not_determined`, thiếu quyền, lỗi ≠ rỗng; ba chỗ đặt
  ở Data; hai chỗ đặt ở People và ba nhóm; cập nhật `?tab=` của link; invalidate query key sau mọi lệnh ghi tag.
- **Bundle guard** `test:bundle:governance-classification`, một marker cho mỗi module của khu.
- **Live** (bắt buộc): stack `dgcl-` cô lập với Trino 483 và StarRocks 4.1.6 thật, dự đoán commit trước (plan Task 19).

## 10. Ngoài #4b

Quyền gắn tag; độ lệch rule-key/engine-name (CL6); phân trang các lần kiểm BI vượt 500; phân giải group và role trên
engine; sửa policy viết tay từ Data hay People (chúng link tới editor).
