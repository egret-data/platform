# Thiết kế lại frontend Data Governance #4a: Engine & policy — thiết kế

> Bản tham khảo tiếng Việt. Bản tiếng Anh `docs/en/specs/2026-10-19-dg-engine-policy-design.md` là bản chuẩn; hai bản
> lệch nhau thì theo bản tiếng Anh.

Ngày: 2026-10-19. Trạng thái: thiết kế đã được người dùng duyệt ngày 2026-10-19 (U1–U11 bên dưới); các dòng ghi
"default decision" là quyết định của tài liệu này. Q1–Q4 là chỗ thiết kế đã duyệt mâu thuẫn với code; chúng đã được
quyết ngày 2026-10-19 (Q1 do người dùng, Q2–Q4 là default decision). Base: main `3731f0ce` (#3 Overview đã merge là #695,
F11 engine grants là #694). Nhánh `feat/dg-engine-policy`, worktree `egret-dgep`.

Đây là sub-project #4a của spec chương trình `docs/en/specs/2026-10-16-dg-frontend-redesign-design.md` (dưới đây gọi
là "spec chương trình"; quyết định F1–F12). Phần 1 của spec đó giao cho nhóm sidebar cuối cùng, **Engine & policy**,
một trang cho mỗi engine gồm Engine permissions (F11), policy biên dịch và policy viết tay, overlap, migrate và trạng
thái tải plugin, thay cho Policies, ProfileDetail, PolicyEditor, PolicyMigration và OverlapReport (F12). Mọi khẳng định
về code hiện có đều kèm `file:line` tại `3731f0ce`, đọc trong worktree này.

## Các tiền đề đã đối chiếu với code

Phần khảo sát trong brief đã được kiểm tra. Các câu sai hoặc thiếu được liệt kê trước; thiết kế đi theo code.

**Sai hoặc thiếu**

1. **"egret-seeded infra|operator".** `managed_by` có ba giá trị `compiler`, `infra`, `operator`
   (`migrations/0180_policy_managed_by.sql:59-63`), và `operator` là mặc định cho policy do người viết trong editor
   (`0180:40-41`, `:59`). Hàng do egret seed chỉ là `infra`. Ngoại lệ duy nhất migration ghi lại: bộ `all - …` dạng
   Trino trên profile StarRocks được để `operator` để operator xoá được (`0180:48-50`); với mọi mục đích của trang này
   nó là policy viết tay. Vì vậy bộ lọc nguồn của tab Policies là *egret-compiled* (`compiler`), *egret-seeded*
   (`infra`), *hand-written* (`operator`).
2. **"impersonate = user picker → targetUserId".** Body ghi mang target dưới dạng **username** trong `values[0]`
   (`NewEngineGrant`, `crates/egret-perm-compile/src/routes/engine_grants.rs:204-215`); server phân giải nó thành id.
   DTO đọc trả cả username hiện tại trong `values` lẫn `targetUserId` (`engine_grants.rs:148-154`); target không có
   username thì bị bỏ khỏi `values`. User picker của dialog trả về username.
3. **"compile failure said plainly ('saved, not yet on the engine')".** Compile thất bại bên trong một lần ghi engine
   grant làm lần ghi thất bại: `write_engine_grant` trả lỗi của recompile trước khi handler commit
   (`engine_grants.rs:887`; handler chỉ commit sau đó, `:966`), và 409 mang lý do từ chối (`:945`). Không có gì được
   lưu. Một grant chỉ có thể được lưu mà không được biên dịch khi compile **bỏ qua** nó (`CompileResponse.skipped`, một
   `SkipReport` có `sourceKind = "engine_grant"` và id của grant, `crates/egret-perm-compile/src/routes.rs:973-981`,
   `:1010-1018`), ví dụ target không còn phân giải được. Grant đã biên dịch chỉ có trên engine sau lần tải kế tiếp của
   plugin, điều chỉ tab Plugin nói được. §6.1 diễn đạt ba kết quả theo đó.
4. **Giá trị `valueKind`.** Trên wire: `username`, `identifier`, `catalog`, `function_signature`, và `fixed` cho key mà
   compiler tự cấp giá trị (`*`; `starrocks.manage_blacklist`) (`crates/egret-perm-compile/src/engine_capabilities.rs:24-44`,
   `engine_grants.rs:85-90`, `:492`). Key `fixed` không nhận input.
5. **Tên policy biên dịch chỉ ra nguồn của nó.** `attribute_policy_name` (`crates/egret-perm-compile/src/attribution.rs:97-116`)
   ánh xạ `perm-grant-{uuid}` → grant, `perm-rule-{uuid}[-locator]` → rule, `perm-clearance-{tag uuid}[-locator]` →
   classification (tag), `perm-clearance-mv` → lệnh deny materialized view toàn engine (không có một nguồn duy nhất), và
   `perm-engine-{uuid}` → engine grant. Suy ra được đối tượng nguồn từ tên trong mọi trường hợp trừ MV deny. Bản sao
   danh sách prefix ở frontend, `isCompilerOwnedPolicyName` (`frontend/src/api/policies.ts:270-271`), chỉ biết hai
   trong bốn prefix; thiết kế này đọc `managed_by` để biết chủ sở hữu và một parser tên mới chỉ để tạo link (§6.2).
   Task 1 của plan đo xem các hàng compiler đang lưu có khớp không.
6. **Policy theo tag nằm ở profile khác.** Policy tag của một profile Trino được lưu ở tag service của nó, một hàng
   `authz_services` riêng do `config.tagService` đặt tên và được trả về là `ProfileDto.tag_service`
   (`crates/egret-policy/src/admin/mod.rs:445-455`; client liệt kê bằng `listPolicies(tag_service)`,
   `frontend/src/routes/governance/ProfileDetailPage.tsx:248-264`). Vì vậy tab Policies liệt kê hai service, và mục
   "Profiles without an engine" không được liệt kê một tag service như profile mồ côi.
7. **Policy editor cần loại policy trong URL.** `PolicyEditorPage` đọc `:kind` (`access`|`masking`|`rowfilter`) từ
   route trước khi policy tải xong (`frontend/src/routes/governance/PolicyEditorPage.tsx:287-299`) và dựng form trống
   cùng resource def từ nó (`:153`, `:349-377`). URL đã duyệt `…/advanced/policies/new` và
   `…/advanced/policies/:policyId` không có kind (Q2).
8. **Route xoá profile đã có client sinh tự động.** `policyAdminDeleteProfile` / `usePolicyAdminDeleteProfile` có sẵn
   (`frontend/src/api/generated/policy-admin/policy-admin.ts:932`, `:982`); lời gọi `apiFetch` và comment của nó ("the
   orval client isn't regenerated for this route yet", `frontend/src/api/policies.ts:97-108`) đã cũ. Code mới dùng hook
   (FE-DATA-2).
9. **Profile của engine không nằm trên `EngineDto`.** `EngineDto` không mang binding (`crates/egret-engine-mgmt/src/engines.rs:290-332`).
   Trang engine đọc profile và reach từ `GET /perm/engines/{id}/grants` (`policy:read`, `engine_grants.rs:195-202`).
   Phiên bản plugin của `EngineDto` cần `engine:read`.
10. **`ProfileDto.engines` thiếu origin và type.** `ProfileEngine` là `{id, name, status}` (`admin/mod.rs:471-475`),
    dựng từ `EngineBinding` (`crates/egret-policy/src/admin/reader.rs:12-22`), vốn không có hai trường đó. `ProfileDto`
    chỉ có tổng `policy_count`, không có số đếm theo chủ sở hữu (`admin/mod.rs:438-444`). Trang index cho người chỉ có
    `policy:read` không thể hiện origin hay số policy viết tay nếu backend không đổi (Q1).
11. **Ledger plugin được đọc ở hai crate, và phán định của Overview không phải là một hàm.** Ngoài
    `governance_overview.rs` (truy vấn ở `:666-674`), `egret-policy` đọc `policy.ranger_plugin_downloads` trong
    `plugins_hold_tag_version` và `wait_for_tag_version` (`crates/egret-policy/src/plugin_downloads.rs:130`, `:175`).
    Luật của Overview nằm inline trong `drift` (`crates/egret-controlplane/src/governance_overview.rs:666-778`) và
    `behind` (`:780-817`), trên các profile có ít nhất một engine provisioned đang chạy (`drift_profiles`, `:632-662`).
    Chúng phán bốn trạng thái chứ không phải ba: silent (không có poll `policies` trong 24 h), quiet (5 phút–24 h, hoặc
    poll tags im trong khi policies vẫn được poll), behind, và còn lại thì không có gì. §4 tách ra một hàm mà cả hai bên
    gọi, và tab Plugin cũng hiện `silent`, nếu không nó sẽ nói khác Overview.
12. **Ledger tính theo profile, không theo engine.** Khoá của nó là `(service_id, plugin_id, kind)`
    (`migrations/0186_plugin_downloads.sql`). Trên một profile chia sẻ kiểu cũ, tab không thể nói một hàng đến từ engine
    nào; nó nói rõ điều đó (O13 của Overview đã để việc gán theo từng engine ra ngoài).
13. **Profile đã compile từ chối policy viết tay MỚI** (đo được, Task 1 phát hiện 1,
    `docs/en/plans/2026-10-19-dg-engine-policy-live.md` §1.1). Lần compile đầu tiên đặt `origin = 'generated'` cho
    profile (cả tag service); từ đó `POST /policies` và `POST /policies/services/{s}/seed-defaults` trả 409
    ("profile N is compiled by the SP4a permission model …", `crates/egret-policy/src/store.rs:543-561`, `:701`), còn
    sửa, bật, tắt và xoá một hàng `operator` có sẵn vẫn thành công (`refuse_if_compiler_owned` chỉ từ chối theo tên,
    `store.rs:619-640`). Vì vậy mọi engine có profile đã compile một grant, rule, clearance hay engine grant đều là
    `generated`, và Create policy cùng Seed default policies của §6.6 chỉ áp dụng cho profile `manual`.
14. **`ProfileDto` dùng snake_case** (đo được, Task 1 phát hiện 2): `GET /policies/services` trả `policy_version`,
    `policy_count`, `tag_service`, `origin` (không có `rename_all`, `admin/mod.rs:437-461`). Các trường thêm của Q1 theo
    đúng kiểu viết đó: `ProfileDto.policy_counts {compiler, infra, operator}`; `ProfileEngine.origin`, `.type`.
15. **Explain capability trả Denied ghi policy là `""`, không phải null** (đo được, M7): `policyName: ""`,
    `decision.policy_name: ""`, `reason: "no_policy_matched"`. Hộp thoại Check coi `""` là không có policy quyết định.

**Đúng như đã nêu** (đã kiểm): route governance ở `frontend/src/App.tsx:615-623` (policies `:615-621`,
classifications `:622-623`); `GOVERNANCE_GROUPS` ở `frontend/src/components/layout/Sidebar.tsx:90-129`, nhóm Policies ở
`:115-118`; `LegacyGovernanceRedirect` và `RetiredPermPageRedirect` trong
`frontend/src/routes/governance/LegacyGovernanceRedirect.tsx`; không có `UNIQUE` trên
`engine_mgmt.engines.authz_service_id` (`migrations/0058_engine_authz_service.sql:5-7`) và I2 từ chối bind chia sẻ mới
(`engines.rs:5311-5326`); `Reach.sharedProfile` (`engine_grants.rs:122-127`); DTO engine grant mang origin
(`engine_grants.rs:100-136`); `EngineDto` có `ranger_plugin_version`/`_modelled`/`_note` qua
`egret_perm::ranger_plugin_report` (`engines.rs:321-366`, `crates/egret-perm/src/plugin_report.rs:29`); chưa có endpoint
plugin theo profile; bảy hook engine grant đã được sinh và chưa có ai gọi (`frontend/src/api/generated/perm/perm.ts`);
trang đối tượng Data ghi "Hand-written and egret-seeded engine policies are not listed here"
(`frontend/src/routes/governance/tabParts.tsx:88`), không đổi trong #4a; migration cuối của main là `0199`. People
picker là `PrincipalPicker` (`frontend/src/components/perm/PrincipalPicker.tsx:114`), sửa một tập `{include, exclude}`;
picker một user là `SubjectUserField` (`frontend/src/components/perm/ExplainResult.tsx:1078`).

## Quyết định

| id | quyết định | người quyết |
|---|---|---|
| EP1 | #4 được tách: **#4a Engine & policy** (spec này); **#4b Classification** cùng một route backend liệt kê các policy viết tay chạm tới một đối tượng Data và dấu "hand-written" trên trang Data/People. #4a không đụng `Classifications.tsx`, `TagDetail.tsx` hay mục sidebar Classification | user, 2026-10-19 (U1) |
| EP2 | Raw Ranger policy editor giữ nguyên quyền hiện nay (`policy:create`/`update`/`delete`, không đổi server). "Công cụ nâng cao chỉ cho admin" nghĩa là chỉ vào được từ tab **Advanced** của trang engine, kèm cảnh báo cố định. Không ai mất quyền | user, 2026-10-19 (U2) |
| EP3 | Nhóm sidebar "Policies" thành **Engine & policy**, nhóm cuối, gate `policy:read`. Index `/governance/engines` (bảng engine; "Profiles without an engine"; tạo/xoá profile, xoá xác nhận bằng cách gõ tên); trang engine `/governance/engines/:id` với các tab Engine permissions · Policies · Overlaps · Migrate · Plugin · Advanced và banner profile chia sẻ từ `reach`; URL cũ redirect; trang cũ bị xoá sau khi có redirect | user, 2026-10-19 (U3) |
| EP4 | Tab Engine permissions và dialog grant trên các route F11; Check qua `explain/capability`; trang principal của People có mục "Engine permissions" | user, 2026-10-19 (U4) |
| EP5 | Tab Policies: mọi policy của profile (và tag service của nó) với bộ lọc nguồn/loại/tìm kiếm; hàng biên dịch chỉ đọc kèm "Open source"; hàng viết tay bật/tắt/xoá và Edit ở Advanced; hàng seed chỉ đọc; giữ `PolicyWarningsCallout`; một dòng đếm theo nguồn | user, 2026-10-19 (U5) |
| EP6 | Tab Overlaps: `GET /perm/overlaps` lọc theo profile | user, 2026-10-19 (U6) |
| EP7 | Tab Migrate: trang migrate hiện nay được restyle, hành vi không đổi, chỉ hiện khi profile có policy viết tay | user, 2026-10-19 (U7) |
| EP8 | Route mới `GET /api/control-plane/governance/engines/{id}/plugin` (`policy:read` VÀ `engine:read`), phán ledger qua **cùng một hàm** với Overview | user, 2026-10-19 (U8) |
| EP9 | Tab Advanced: editor đã restyle, cảnh báo cố định, danh sách policy viết tay, Create, Seed defaults (Trino, `policy:seed`) | user, 2026-10-19 (U9) |
| EP10 | Kiểm thử như §9, gồm kiểm tra sống với Trino và StarRocks thật, dự đoán trước | user, 2026-10-19 (U10) |
| EP11 | Không dự kiến migration. Nếu cần, số được chọn bằng `ls migrations \| tail` ngay trước khi merge | user, 2026-10-19 (U11) |
| EP12 | Trang engine nằm dưới Governance, không phải một tab của trang engine ở Control Plane; hai bên link sang nhau | default decision (2026-10-19) |
| EP13 | Index có mục "Profiles without an engine"; trang profile có các tab của trang engine trừ Engine permissions | default decision (2026-10-19) |
| EP14 | Profile chia sẻ (kiểu cũ, nhiều engine) hiện banner trên mọi tab, nêu tên các engine khác | default decision (2026-10-19) |
| EP15 | Trang profile là `/governance/engines/profiles/:service`. Khi profile đã bind, nó redirect (replace) sang trang engine; một resolver này phục vụ `/governance/policies/:service` và các link profile của Overview | default decision (2026-10-19) |
| EP16 | Hàm phán định chuyển sang `crates/egret-controlplane/src/plugin_judgement.rs`; câu chữ và finding của Overview không đổi (test của nó vẫn xanh, không sửa) | default decision (2026-10-19) |
| EP17 | Trạng thái của tab Plugin theo từng kind: `up_to_date`, `pending`, `behind`, `quiet`, `silent`, `not_reported` (hai lý do), và `not_judged` cho profile (external, không chạy, không có profile) | default decision (2026-10-19); thêm `silent` để tab không bao giờ nói khác Overview (tiền đề 11) |
| EP18 | "Download Trino config" (`TrinoConfigDialog`) chuyển sang tab Plugin | default decision (2026-10-19) |
| EP19 | Trang engine gate `policy:read`. Không có `engine:read` thì tab Plugin và phiên bản plugin ở header được thay bằng một dòng nêu tên quyền; mọi tab khác vẫn hoạt động | default decision (2026-10-19) |
| EP20 | People: mục "Engine permissions" nằm cuối tab Permissions của trang principal, không phải tab mới | default decision (2026-10-19) |
| EP21 | `PrincipalPicker` có thêm prop `includeOnly` (không có danh sách Exclude), theo kiểu cộng thêm; dialog grant dùng nó | default decision (2026-10-19) |
| EP22 | **Kind mà engine không được kỳ vọng tải được phán `not_expected`, và chỉ ở `judge_plugins`** (sửa theo review D1). Tiền đề: plugin của một profile chỉ có tag policy để tải khi profile nêu một tag service phân giải được (`egret_policy::usable_tag_services_via`, đúng phép join mà `ProfileDto.tag_service` và bản tải plugin đọc); CHECK của migration 0176 giữ `tagService` khỏi mọi profile không phải Trino, nên mọi profile StarRocks và profile Trino không có tag service đều không kỳ vọng tags. Kind tags của profile như vậy là `not_expected` bất kể các hàng nói gì (hàng thô vẫn được liệt kê), với câu "Profile P has no tag service, so its plugins are not expected to download tags." (thêm " StarRocks never downloads tags." trên engine StarRocks). Chọn cách này thay vì bỏ kind: thiếu hàng thì tab im lặng về lý do tags không được phán, còn câu trung thực là egret không kỳ vọng gì, chứ không phải không có gì xảy ra. Overview không nêu drift tags nào cho profile như vậy; index xếp `not_expected` cuối cùng và không giữ luật riêng theo loại engine. Điều này sửa EP16: các test Overview có fixture drift tags không nêu tag service nay nêu một tag service (assertion không đổi), và mỗi test có thêm một fixture ghim trường hợp không có tag service | default decision (2026-10-19, vòng sửa review) |
| Q1 | **Dữ liệu index cho người chỉ có `policy:read`** (tiền đề 10): response `GET /policies/services` hiện có (vốn đã `policy:read`) được thêm `ProfileEngine.origin`, `ProfileEngine.type` và `ProfileDto.policyCounts {compiler, infra, operator}` (profile cộng tag service của nó), theo kiểu cộng thêm; không thêm route. Cột plugin của index chỉ hiện khi có `engine:read`, mỗi engine một lần gọi plugin | user, 2026-10-19 |
| Q2 | URL editor giữ segment kind (tiền đề 7): `…/advanced/policies/:kind/new` và `…/advanced/policies/:kind/:policyId` | default decision (2026-10-19) |
| Q3 | URL cũ `/governance/policies/:service` nêu một profile bind với nhiều engine (chia sẻ kiểu cũ) redirect tới engine đầu tiên theo tên (thứ tự `reach`); banner của nó nêu các engine còn lại | default decision (2026-10-19) |
| Q4 | Câu chữ kết quả ghi theo code (tiền đề 3): **not saved** cho mọi lời từ chối, kể cả 409 do compile thất bại (lần ghi bị rollback; dialog vẫn mở với lý do của server); **saved but not compiled** khi `compile.skipped` nêu grant ("Saved, but not compiled: <reason>. It is not on the engine."); **compiled** trong các trường hợp còn lại ("Saved and compiled into <profile>. The engine applies it at its next plugin download.") | default decision (2026-10-19) |

## 1. Những gì đang có

- **Route và trang.** `/governance/policies` (`Policies.tsx`: danh sách profile, New profile, Delete profile với
  `confirmText="delete"` `:342-350`, `OverlapReport` liên profile `:287`); `/governance/policies/:service`
  (`ProfileDetailPage.tsx`: header, Seed defaults `:161-177`, link Migrate `:181`, Download Trino config `:188-193`,
  `PolicyWarningsCallout` `:224`, `OverlapReport serviceName` `:226`, các tab Access/Masking/Row-filter/Tags
  `:228-268`); `…/migrate` (`PolicyMigrationPage.tsx`, param `service` `:68`, ghi cần `policy:update` `:71-73`);
  `…/:kind/new` và `…/:kind/:id` (`PolicyEditorPage.tsx`, chế độ resource và by-tag, `?scope=tag`). Tất cả nằm sau
  `RequirePermission perm="policy:read"` (`App.tsx:615-621`). URL Control Plane cũ tới đây qua
  `LegacyGovernanceRedirect` (`App.tsx:656`).
- **Link trỏ vào chúng.** `ProfileDetailPage.tsx:94,181,262,376,381`, `PolicyMigrationPage.tsx:280`,
  `OverlapReport.tsx:123,183`, `Policies.tsx:120`, `PolicyEditorPage.tsx:388,1174`, `PolicyWarningsCallout.tsx:115`,
  `overview/refUrl.ts:27,29`, `control-plane/externalEngine.tsx:192-198` (`PolicyProfileLink`, dùng ở
  `ExternalEngineDetail.tsx:265`), `control-plane/access/AccessOverview.tsx:61`.
- **Route F11** (`engine_grants.rs`): `GET /perm/engine-capabilities` (`:463`), `GET /perm/engines/{engine_id}/grants`
  (`:513`; `engine`, `profile`, `reach`, `grants`, `managed`), `GET /perm/engine-grants` (`:579`), `POST
  /perm/engines/{engine_id}/grants` (`:935`), `PATCH`/`DELETE /perm/engine-grants/{id}` (`:979`, `:1025`), `POST
  /perm/explain/capability` (`:1192`). Đọc và explain cần `policy:read`, ghi cần `policy:update`. Mã trạng thái theo
  F11 §5.5. `ManagedKind` = `user_baseline` | `delegator` | `admin_all_access` | `default_all_access` (`:162-176`).
- **Danh sách policy.** `GET /policies?service=&type=` trả `PolicySummary` có `managed_by`
  (`admin/mod.rs:677-689`, `frontend/src/api/policies.ts:57-72`); `PUT /policies/{id}/enabled`, `DELETE /policies/{id}`
  từ chối phía server với hàng `infra` và hàng thuộc compiler.
- **Overlap.** `GET /perm/overlaps` (`usePermListOverlaps`), lọc theo profile bởi `OverlapReport` (`OverlapReport.tsx:45`).
- **Plugin.** Ledger và luật của Overview (tiền đề 11–12); `TrinoConfigDialog`
  (`frontend/src/components/policy/TrinoConfigDialog.tsx`).
- **Component picker.** `PrincipalPicker`, `SubjectUserField` (danh sách tiền đề); `useControlPlaneGovernanceListCatalogs`
  trả mỗi catalog cùng tên các engine nó được gắn vào (`crates/egret-controlplane/src/governance_browse.rs:70-79`).

## 2. Kiến trúc thông tin

| URL | trang | guard |
|---|---|---|
| `/governance/engines` | index | `policy:read` |
| `/governance/engines/:id?tab=` | trang engine; `tab` ∈ `engine-permissions` (mặc định), `policies`, `overlaps`, `migrate`, `plugin`, `advanced` | `policy:read` |
| `/governance/engines/:id/advanced/policies/:kind/new` · `…/:kind/:policyId` | editor, cho profile của engine (Q2) | `policy:read` (ghi theo từng control, như hiện nay) |
| `/governance/engines/profiles/:service?tab=` | trang profile (profile chưa bind); redirect sang trang engine khi đã bind (EP15) | `policy:read` |
| `/governance/engines/profiles/:service/advanced/policies/:kind/new` · `…/:kind/:policyId` | editor cho profile chưa bind | `policy:read` |

Segment tĩnh `profiles` xếp hạng trên `:id` trong react-router; một test ghim điều này. Mọi thứ nằm trong một vùng lazy
`frontend/src/routes/governance/engines/` (`EnginesRoutes.tsx`), mount trong `App.tsx` bên trong `LazyRouteArea`
(FE-ROUTE-2), với bundle guard `npm run test:bundle:governance-engines`
(`frontend/scripts/assert-governance-engines-lazy.mjs`, liệt kê trong FE-ROUTE-2 và FE-TEST-4).

Sidebar: bỏ nhóm `{ heading: "Policies", … }` (`Sidebar.tsx:115-118`) và thêm nhóm cuối
`{ heading: "Engine & policy", items: [{ label: "Engine & policy", icon: ShieldCheck, perm: "policy:read", to: "engines" }] }`
sau Lineage. Heading này khác mọi heading của app khác (`Sidebar.tsx:79-80`).

## 3. Redirect và link

| URL cũ | URL mới |
|---|---|
| `/governance/policies` | `/governance/engines` |
| `/governance/policies/:service` (và `?tab=access`, `masking`, `rowfilter`, `tags`) | `/governance/engines/profiles/:service?tab=policies` → resolver (EP15, Q3); tab loại cũ thành bộ lọc loại của tab Policies (`&type=`) |
| `/governance/policies/:service/migrate` | resolver với `tab=migrate` |
| `/governance/policies/:service/:kind/new` (giữ `?scope=tag`) | resolver tới `…/advanced/policies/:kind/new` |
| `/governance/policies/:service/:kind/:id` | resolver tới `…/advanced/policies/:kind/:id` |
| `/control-plane/policies/*` | đã → `/governance/policies/*` (`LegacyGovernanceRedirect`), rồi theo các dòng trên |

`PolicyRedirect` (mới, `routes/governance/engines/PolicyRedirect.tsx`) đọc danh sách profile (`listProfiles`), tìm
profile theo tên, và điều hướng với `replace`: đã bind → `/governance/engines/<engine id>/…`, chưa bind →
`/governance/engines/profiles/<name>/…`, không tồn tại → index kèm thông báo "No policy profile named X". Nó giữ
`search` và `hash`. Nó import tĩnh được (không import trang nào một cách eager) nên các route redirect ở lại `App.tsx`.

Link được cập nhật: `overview/refUrl.ts` (`profile` → `/governance/engines/profiles/<name>`; `profiles` →
`/governance/engines`; `refUrl` có thêm tham số tuỳ chọn `tab` mà `AttentionRow` truyền: `plugin` cho `plugin_*`,
`policies` cho `policy_warnings`, `overlaps` cho `rule_overlaps`; ref engine vẫn trỏ về Control Plane, nơi sửa
`engine_failed` và `plugin_unmodelled`); `PolicyProfileLink` (→ resolver); `AccessOverview.tsx:61` (→
`/governance/engines`); header của `EngineDetail` và `ExternalEngineDetail` ở Control Plane có thêm "Open in Engine &
policy" → `/governance/engines/:id`, hiện khi có `policy:read` (EP12).

## 4. Backend: route plugin

### 4.1 Một hàm phán định (EP8, EP16)

`crates/egret-controlplane/src/plugin_judgement.rs`:

```rust
pub(crate) struct ProfileVersions { pub policy_version: i64, pub tag_version: i64, pub updated_at: DateTime<Utc>,
                                    pub expects_tags: bool }   // EP22
pub(crate) struct LedgerRow { pub service_id: i64, pub plugin_id: String, pub kind: String,
                              pub last_known_version: Option<i64>, pub served_version: Option<i64>, pub at: DateTime<Utc> }
pub(crate) enum Liveness { Live, Quiet { age_secs: i64 }, Silent }
pub(crate) enum NotReported { NoDownload, VersionUnreported }
pub(crate) enum KindState { UpToDate, Pending, Behind { version: i64 }, Quiet { age_secs: i64 }, Silent,
                            NotReported(NotReported), NotExpected }   // NotExpected: EP22
pub(crate) struct Judgement { pub liveness: Liveness, pub policies: KindState, pub tags: KindState }
pub(crate) fn judge_plugins(p: &ProfileVersions, rows: &[&LedgerRow], now: DateTime<Utc>) -> Judgement;
pub(crate) async fn ledger_rows_via(c: &mut PgConnection, service_ids: &[i64]) -> Result<Vec<LedgerRow>, sqlx::Error>;
```

Luật là luật của Overview, được chuyển chứ không viết lại (`QUIET_SECS`, `SILENT_SECS`, `BEHIND_SECS`, liveness theo
poll mới nhất, luật `-1`, cửa sổ 5 phút cho `behind`, tags quiet của review M6). Ánh xạ: kind tags mà profile không
kỳ vọng (EP22) là `NotExpected` trước mọi luật khác; khi liveness là `Silent` hoặc
`Quiet`, cả hai kind mang trạng thái đó; ngược lại mỗi kind là `NotReported(NoDownload)` khi không có hàng nào của kind
đó trong 24 h, `Quiet` cho kind tags im trong khi policies vẫn được poll, `Pending` khi `updated_at` chưa quá 90 s,
`Behind` như `behind` quyết định hiện nay, `UpToDate` khi một hàng poll lúc hoặc sau U trong vòng 5 phút báo ≥ V, còn lại
là `NotReported(VersionUnreported)` (đã poll từ U, mọi báo cáo đều `-1`). `drift` gọi `judge_plugins` và dựng finding từ
kết quả: `plugin_silent` ⇔ `Silent`, `plugin_quiet` ⇔ liveness `Quiet` hoặc tags `Quiet`, `plugin_behind` ⇔ một kind
`Behind` (kind đầu tiên, policies trước tags, như hiện nay). `UpToDate` và `VersionUnreported` đều là "không có
finding", như hiện nay; chỉ tab phân biệt hai trạng thái này.

### 4.2 `GET /api/control-plane/governance/engines/{id}/plugin`

Trong file mới `crates/egret-controlplane/src/governance_engines.rs`, tag `control-plane-governance`. Cần `policy:read`
**và** `engine:read` (hai lời gọi `egret_core::authz::require`; thiếu một trong hai thì 403). Một transaction
`REPEATABLE READ, READ ONLY`, như `begin_snapshot` (`governance_overview.rs:359`).

```text
{ generatedAt,
  engine: { id, name, type, origin, status },
  profile: { serviceId, name, policyVersion, tagVersion, updatedAt } | null,
  sharedWith: [ { id, name, origin } ],                     // các engine khác bind cùng profile, theo tên
  plugin: { version: string|null, modelled: bool, note: string|null },   // egret_perm::ranger_plugin_report
  judged: { state: "judged" } | { state: "not_judged", reason: "external" | "not_running" | "no_profile" },
  kinds: [ { kind: "policies"|"tags", currentVersion, lastDownloadAt|null, newestReportedVersion|null,
             servedVersion|null, state: "up_to_date"|"pending"|"behind"|"quiet"|"silent"|"not_reported"|"not_expected",
             notReported: "no_download"|"version_unreported"|null, sentence } ],   // rỗng khi không phán
  rows: [ { pluginId, kind, lastKnownVersion, servedVersion, at } ] }              // thô, mới nhất trước
```

- `not_judged`: `external` khi `origin = 'external'` (câu "egret does not judge external engines: it does not run them
  and cannot tell whether they should be polling."); `not_running` khi profile không có engine provisioned nào với
  `status = 'running'` (đúng tập của Overview, `drift_profiles`); `no_profile`. `rows` được trả trong mọi trường hợp
  trừ `no_profile`.
- Câu chữ: dùng câu của Overview cho cùng trạng thái khi Overview có câu (`behind`, `quiet`, `silent`); các trạng thái
  khác: "Plugins of profile P report holding policy version V." / "A plugin downloaded version V at T; confirmation
  takes two polls." / "No plugin of profile P downloaded tags in 24 hours." / "Profile P has no tag service, so its plugins are
  not expected to download tags. StarRocks never downloads tags." (`not_expected`, EP22; vế sau chỉ cho engine StarRocks) / "Plugins polled since version V was set, but none has reported a version yet." Không bao giờ
  dùng "enforces" (ledger ghi lại những gì plugin báo, `plugin_downloads.rs:12-14`).
- 404 cho engine không tồn tại. Profile chia sẻ (`sharedWith` không rỗng) được trả lời theo profile; tab nói rằng các
  hàng có thể đến từ bất kỳ engine nào trong số đó.
- Không migration: việc đọc theo profile được khoá chính `(service_id, plugin_id, kind)` phục vụ (EP11).

## 5. Index (`/governance/engines`)

`ListPageShell` (FE-PAGE-1): `h1` "Engine & policy", toolbar (New profile, `policy:create`), `SummaryStrip` (engines ·
profiles without an engine · hand-written policies, mục cuối chỉ khi đếm được, Q1), rồi hai mục.

**Engines** (`FilterableTable`): tên (link sang trang engine), loại, origin (`provisioned`/`external`), profile (tên,
hoặc "No profile" kèm link sang Settings của engine ở Control Plane khi có `engine:read`), trạng thái plugin (trạng thái
kind tệ nhất từ route plugin, chỉ khi có `engine:read`; "Not judged: external" cho engine external), engine grants (số
hàng `GET /perm/engine-grants` được ghi trên engine đó), policy viết tay (Q1). Nguồn: có `engine:read` thì
`GET /control-plane/engines` (mọi engine, kể cả chưa bind); không có thì các engine đã bind trong `ProfileDto.engines`.
Rỗng: "No engine is recorded. Engines are created in Control plane."

**Profiles without an engine**: các profile có `engines` rỗng và không bị profile nào khác gọi là `tag_service` (tiền
đề 6). Mỗi hàng: tên (link sang trang profile), loại, origin (`manual`/`generated`), số policy, Delete
(`policy:delete`; `ConfirmDeleteDialog` với `confirmText` = tên profile; hiện 409 của server, ví dụ profile
generated). Rỗng: "Every profile is bound to an engine."

New profile: dialog hiện có (tên, loại), chuyển từ `Policies.tsx`. Delete dùng `usePolicyAdminDeleteProfile` (tiền đề 8).

## 6. Trang engine (`/governance/engines/:id`) và trang profile

**Header.** Tên; badge loại; badge origin; tên profile (hoặc "No profile"); phiên bản plugin và ghi chú "not modelled"
(từ route plugin, EP19); "Open in Control plane" → `/control-plane/engines/:id` khi có `engine:read`. Dữ liệu:
`usePermListEngineGrants(id)` (engine, profile, reach). 404 → "No engine with this id" và link về index.

**Banner profile chia sẻ** (EP14), trên mọi tab, từ `reach.sharedProfile`: "Profile P is shared with engines A, B.
Everything on this page applies to all of them." Engine external (trên tab có thao tác ghi): "egret compiles this
engine's policies but cannot see whether the external engine downloads them."

**Tab** (`ui/tabs`, `?tab=`, FE-PAGE-2): Engine permissions · Policies · Overlaps (badge đếm) · Migrate (chỉ khi
profile có policy viết tay) · Plugin · Advanced. Tab được nêu trong `?tab=` mà không hiển thị thì rơi về tab đầu.
**Trang profile** dùng cùng khung cho profile chưa bind: header (tên, loại, origin, "No engine is bound to this
profile"), các tab Policies · Overlaps · Migrate · Advanced (không có Engine permissions, không có Plugin).

Mỗi tab có skeleton tải, lỗi (thông điệp server, Retry) và trạng thái rỗng riêng; một request lỗi không bao giờ làm
trắng cả trang (FE-STATE-1–3).

### 6.1 Engine permissions (EP4)

- **Không có profile.** "This engine downloads no policy profile, so nothing can be granted on it. Bind a profile in
  Control plane." Không có nút Grant.
- **Đã cấp**, nhóm theo capability (label và description từ `usePermListEngineCapabilities`): mỗi hàng hiện các giá trị
  (target impersonation là username của nó, link sang trang user ở People; "a user with no username" khi `values` không
  có), các principal (mỗi cái link qua `principalUrl`), tên policy sinh ra, và "written on engine X" khi `engineId` ≠
  engine này (profile chia sẻ). Edit và Delete chỉ khi có `policy:update`; Delete qua `ConfirmDeleteDialog`
  (FE-ACTION-1). Rỗng: "No engine permission has been granted on this engine."
- **Managed by egret** (chỉ đọc, từ `managed`): nhãn kind (`user_baseline` "Every user's baseline", `delegator`
  "egret's delegator", `admin_all_access` "Platform admins: all access", `default_all_access` "Group admin: all access
  (seeded defaults)"), nhãn capability khi ánh xạ được, resources, accesses, users và groups, enabled.
- **Not grantable here** (từ `notGrantable` của loại engine): cái gì và vì sao, cùng "A hand-written policy in Advanced
  can grant it." kèm link sang tab Advanced.
- **Dialog grant** (tạo và sửa dùng chung; dialog theo FE-PAGE-3, FE-FORM-1–4): Capability (`Select`, các capability
  của loại engine, label + description) → mỗi phần tử `resources[]` một control theo thứ tự: `username` →
  `SubjectUserField`; `catalog` → `Select` các catalog gắn vào một engine trong `reach`
  (`useControlPlaneGovernanceListCatalogs`, Task 1 đo xem nó có bằng những gì server chấp nhận không); `identifier` →
  `Input` với luật làm trợ giúp; `function_signature` → `Input` với "e.g. my_udf(INT,VARCHAR)"; `fixed` → không có
  control, "Applies to all (*)". Kiểm phía client trước khi gửi: không rỗng, không có khoảng trắng đầu/cuối, không có
  `* ? { }` (luật của server, F11 §2; server vẫn là bên quyết). Principals → `PrincipalPicker includeOnly` (EP21),
  user/group/role; `public` bị từ chối ngay cạnh field bằng câu của server. Sửa gửi cả body (`PATCH`; engine cố định).
  Lỗi 4xx hiện thông điệp server trong dialog, dialog vẫn mở và giữ mọi input (FE-ACTION-3). Thành công thì đóng
  dialog, invalidate grant của engine, mọi engine grant và policy của profile, và báo theo Q4: toast "Saved and
  compiled into P." cộng thêm, khi `reach` nêu engine khác, "It reaches A, B."; khi `compile.skipped` nêu grant này,
  một Alert trên tab "Saved, but not compiled: <reason>. It is not on the engine."; với engine external thì câu banner
  ở trên.
- **Check** (nút trên toolbar, `policy:read`): dialog gồm user (`SubjectUserField`), capability và giá trị (cùng các
  control) → `POST /perm/explain/capability`. Kết quả: "Allowed" / "Denied" / "Cannot be decided ahead of time" (không
  bao giờ chỉ bằng màu, FE-A11Y-4), tên policy quyết định và nguồn của nó: *egret engine grant* (attribution
  `engine_grant`, link tới hàng), *egret-seeded* (tên có trong `managed`), *egret-compiled* (prefix compiler khác), hoặc
  *hand-written*. Undecidable liệt kê nguyên nhân; trên profile Trino ủy quyền thì dùng câu impersonation của F11 §5.4.
  `no_profile` → "This engine has no profile to ask."; `tagResolution = declined` → "egret cannot resolve
  classification tags for this question."

### 6.2 Policies (EP5)

Dữ liệu: `GET /policies?service=<profile>` và, khi có `tag_service`, `?service=<tag_service>`. Bộ lọc nằm trong URL
(`?source=compiler|infra|operator&type=access|masking|rowfilter|tag&q=`), FE-PAGE-2. Một dòng đếm: "N egret-compiled ·
N egret-seeded · N hand-written" (số nhiều viết sẵn, FE-COPY-4).

- **egret-compiled** (`managed_by = 'compiler'`): chỉ đọc; icon khoá và "Compiled by egret; change its source". "Open
  source" từ hàm mới `compiledSource(name)` trong `engines/compiledSource.ts`, bản sinh đôi phía frontend của
  `attribute_policy_name` (tiền đề 5): grant → `/governance/grants/:id`; rule → `/governance/rules/:id`; clearance →
  `/governance/classifications/:tagId` (chỉ hiện khi có `tag:read`, nếu không thì chỉ ghi loại); engine grant → tab
  Engine permissions của engine này với `&grant=:id` (hàng được làm nổi); MV deny và tên không parse được → chỉ ghi
  loại ("Materialized-view deny", "Compiled policy").
- **egret-seeded** (`infra`): chỉ đọc, "Managed by egret (engine delegation, admin access or query limits)."
- **hand-written** (`operator`): nhãn "Not managed by egret"; bật/tắt (`policy:update`), Delete (`policy:delete`, xác
  nhận), Edit → URL editor ở Advanced.
- `PolicyWarningsCallout` ở trên bảng, không đổi.
- Rỗng: "This profile has no policy." Lọc không trúng: "No policy matches these filters." kèm Clear (FE-STATE-3).

### 6.3 Overlaps (EP6)

`usePermListOverlaps`, các hàng có `serviceName` là profile này. Mỗi cặp: hai rule (link tới trang rule, hoặc link
editor của policy viết tay như hiện nay, `OverlapReport.tsx:183`), đối tượng chung, và lý do. Badge đếm trên tab.
Rỗng: "No overlapping rules on this profile." Phần hiển thị chuyển từ `OverlapReport.tsx` sang
`engines/tabs/OverlapsTab.tsx`; sau đó `OverlapReport` bị xoá cùng `Policies.tsx`.

### 6.4 Migrate (EP7)

Thân của `PolicyMigrationPage` thành `engines/tabs/MigrateTab.tsx`, nhận `service` qua prop thay vì route param;
classify, dry run, apply và undo không đổi; restyle theo khung tab (không có link quay lại, không `h1`). Chỉ hiện khi
danh sách Policies có một hàng viết tay. Ghi cần `policy:update` (kiểm theo từng control như hiện nay). Sau apply:
"See it in Recent changes" → `/governance#recent-changes` (mục đó được thêm `id`, theo kiểu cộng thêm).

### 6.5 Plugin (EP8, EP17, EP18)

Từ §4.2. Không có `engine:read`: "Plugin state needs the engine:read permission." (EP19). Ngược lại: phiên bản plugin,
"modelled" hoặc ghi chú; mỗi kind một hàng: chữ và icon trạng thái, câu chữ, lần tải cuối (tương đối + `<time>` tuyệt
đối), phiên bản hiện tại, phiên bản báo mới nhất, phiên bản đã phục vụ; luật trong một dòng ("A version counts as held
once a plugin reports it; confirmation takes two polls, about a minute at the default 30 s poll."). `not_judged` → câu
của nó và chỉ các hàng thô. Bảng hàng thô (plugin id, kind, last known, served, khi nào). Profile chia sẻ: "These rows
may come from any engine bound to P." "Download Trino config" (profile Trino), như hiện nay.

### 6.6 Advanced (EP2, EP9)

Cảnh báo cố định (một `Alert`, luôn hiện): "Hand-written policies are outside egret's model: egret does not recompile
them, does not check overlaps with grants, and Data and People pages will mark them hand-written. Prefer Engine
permissions or grants on Data." Bên dưới: các policy viết tay (tên → editor, loại, resource, enabled), "Create policy"
(menu loại Access/Masking/Row filter, và By tag khi profile có tag service; `policy:create`), và **Seed default
policies** (chỉ profile Trino, `policy:seed`) với "Adds egret's default Trino set (group `admin`: all access). Existing
policies are not changed." Create policy và Seed default policies chỉ được đưa ra trên profile `manual` (tiền đề
13); trên profile `generated` tab thay vào đó nói: "egret compiles this profile, so new hand-written policies are
refused. Existing ones can still be edited, enabled, disabled and deleted." Editor (`PolicyEditorPage`, chuyển sang `engines/PolicyEditorPage.tsx`, hành vi không đổi)
lấy `service` từ route cha và dựng link quay lại cùng điều hướng sau khi lưu về tab Advanced.

## 7. People: Engine permissions trên trang principal (EP4, EP20)

Cuối tab Permissions (`people/tabs/PermissionsTab.tsx`), cho trang user, group và role (không phải public, không phải
workspace): `usePermListAllEngineGrants`, lọc bằng `setReach({include: g.principals, exclude: []}, standsFor)`
(`people/reach.ts`), để grant tới user qua một group được đánh dấu "through group g" đúng như data grant. Mỗi hàng:
engine (link sang trang engine), nhãn capability, giá trị, cách nó chạm tới. Rỗng: "No engine permission reaches this
<user|group|role>."

## 8. Bị xoá

Sau khi redirect có hiệu lực: `Policies.tsx`, `ProfileDetailPage.tsx`, `PolicyMigrationPage.tsx` (thân đã chuyển),
`PolicyEditorPage.tsx` (đã chuyển), `OverlapReport.tsx` (đã chuyển), test của chúng (được port, không bỏ: mọi assertion
còn mô tả hành vi đều chuyển sang file mới), `isCompilerOwnedPolicyName` và `deleteProfile` trong `api/policies.ts` khi
không còn ai gọi. `TrinoConfigDialog` và `PolicyWarningsCallout` được giữ.

## 9. Kiểm thử (EP10)

- **Test DB backend** (`egret_test_db::url()`, fixture giới hạn trong id của chính nó): `judge_plugins` trên mọi trạng
  thái của §4.1 gồm `-1`, plugin id cũ, tags quiet, profile StarRocks và profile Trino không có tag service
  (`not_expected`, EP22); route cho profile được phán,
  external (`not_judged`, có rows), không chạy, không có profile, engine không tồn tại (404), 403 khi thiếu
  `engine:read`, 403 khi thiếu `policy:read`. **Test đồng thuận**: mỗi kind drift của Overview một fixture, khẳng định
  `attention_via` phát `plugin_silent`/`plugin_quiet`/`plugin_behind` đúng khi route báo `silent`/`quiet`/`behind` cho
  profile đó; và `governance_attention.rs` hiện có của Overview qua mà không sửa.
- **Vitest frontend**, đỏ trước theo từng task: route và redirect của mọi URL cũ (đã bind, chưa bind, không tồn tại,
  chia sẻ, với `?scope=tag` và `?tab=`); thứ tự nhóm sidebar, nhãn và gate; `refUrl` và tab của `AttentionRow`; các mục
  của index, loại tag service, xoá bằng gõ tên; loading/rỗng/lỗi của từng tab; Engine permissions: hàng theo nhóm, mục
  managed và not-grantable, control ghi bị ẩn khi thiếu `policy:update`; control của dialog grant theo `valueKind`, kiểm
  phía client, lời từ chối của server giữ trong dialog đang mở, ba kết quả của Q4; ba kết quả của Check; bộ lọc Policies,
  `compiledSource` cho mọi prefix và một tên hỏng, control theo nguồn; bộ lọc Overlaps; tab Migrate chỉ hiện khi có hàng
  viết tay; các trạng thái Plugin và `not_judged`; cảnh báo Advanced, Seed chỉ trên Trino với `policy:seed`; mục People
  với một grant qua group.
- **Bundle guard** `test:bundle:governance-engines`, mỗi module của vùng một marker.
- **Kiểm tra sống** (bắt buộc): một stack cô lập với Trino 483 và StarRocks 4.1.6 thật, dự đoán commit trước (Task 17 của
  plan): cấp impersonation qua dialog và chứng minh trên từng engine bằng một truy vấn; impersonate `root` và một
  platform admin bị từ chối với lý do của server; Check trả "cannot be decided ahead of time" cho impersonation Trino
  trên profile ủy quyền; tab Plugin và Overview nói cùng một điều khi StarRocks dừng (quiet) và sau một lần compile
  (pending → up to date); mọi redirect; chỉ dùng bàn phím, Escape đóng từng dialog, 390 px và 1280 px, sáng và tối.

## 10. Ngoài #4a

Classification và TagDetail (#4b); route liệt kê policy viết tay theo đối tượng Data và dấu "hand-written" (#4b); gán
plugin theo từng engine; engine grant dạng deny; sửa `tagService` của profile; RisingWave.
