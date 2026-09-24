# SP3c — Catalogs UI, và việc khai tử thế giới định hình theo warehouse

**Trạng thái:** đã thiết kế
**Ngày:** 2026-09-22
**Mở rộng:** `2026-09-19-catalog-registry-design.md` §5 (SP3a đã merge trong #587, SP3b trong #589)
**Được triển khai thành hai PR:** SP3c-1 là UI, SP3c-2 là việc gỡ bỏ.

SP3a biến `registry.catalogs` thành nguồn chân lý (source of truth); SP3b dạy cho StarRocks đọc
được nó. Cả hai đều giữ cho các màn hình hiện tại tiếp tục hoạt động bằng cách reimplement các
endpoint cũ trên nền registry. SP3c là nơi các màn hình bắt kịp, và sau đó là nơi các shape cũ ra
đi.

## 1. Những gì code nói mà thiết kế thì không

Ba điều được phát hiện bằng cách đọc cây mã nguồn (tree), và mỗi điều đều làm thay đổi kế hoạch:

- **`WarehouseDetail.tsx` dài 2379 dòng và các tab của nó không phải như §5 đã giả định.** Chúng là
  Roles & grants, Catalog Clients, Engines, Browse, Connect và Settings — UI duy nhất ở bất cứ đâu
  cho catalog roles, grants, clients, namespace browsing và cấu hình warehouse. Xoá trang này mà
  không di dời (relocate) chúng là xoá cả một management surface (bề mặt quản trị), chứ không phải
  một màn hình.
- **`engines.warehouse_id` mang tải trọng (load-bearing).** Nó quyết định attachment nào là catalog
  *primary* của một engine (`list_engine_attachments(engine.id, Some(engine.warehouse_id))`) và
  seed Ranger role của engine. Đây không phải là legacy cruft để loại bỏ; "primary" phải được định
  nghĩa ở một nơi khác trước đã.
- **`warehouse:*` gate (chặn) ~1550 dòng của `warehouse_admin`** — roles, grants, clients, browse,
  config, protection, format policy — cộng thêm các endpoint resource-tag phân loại
  (classification). `catalog_registry:*` không bao phủ bất kỳ phần nào trong số đó. Và
  `warehouse:read` là một trong tám key trong `CONTROL_PLANE_PERMS`, thứ quyết định liệu một user
  có được vào Control Plane **hay không**: gỡ bỏ nó một cách cẩu thả sẽ khoá người dùng ra khỏi
  nhiều thứ hơn là một trang.

## 2. Các quyết định

**D1 — Một trang catalog detail duy nhất, với các tab có điều kiện theo kind.** Configuration,
Engines và Workspaces cho mọi catalog; một catalog iceberg-egret hiển thị thêm các tab warehouse
hiện nay, tái sử dụng (reuse) các component đó thay vì viết lại chúng. Một catalog jdbc đơn giản là
có ít tab hơn.

**D2 — Primary trở thành một flag trên attachment.** `registry.engine_catalog_attachments` được
thêm `is_primary boolean`, duy nhất theo từng engine, được backfill từ `engines.warehouse_id`; sau
đó cột này bị xoá trong SP3c-2. ~~SP3c-2 cũng sở hữu việc cho một catalog không-warehouse dẫn đầu.~~
**Rút lại 2026-09-23 (quyết định của người dùng): attachment primary bắt buộc trỏ tới một catalog có
warehouse, trong SP3c-2 cũng như hôm nay** — xem §4 bước 3 để biết vì sao đó là một tính năng chứ
không phải một phép viết lại cơ học. Primary là một thuộc tính của *quan hệ (relationship)* giữa một engine và một
catalog — điều mà nó vốn luôn là như vậy — lưu nó trên engine chính là thứ đã ép buộc một warehouse
cho mỗi engine.

**D3 — Một họ permission (permission family) duy nhất.** `warehouse_admin` và các endpoint
resource-tag chuyển sang `catalog_registry:*`, và một migration cấp
`catalog_registry:{read,create,update,delete}` cho mọi role đang giữ `warehouse:*` tương ứng,
trước khi các key cũ bị bỏ. Không ai được thêm quyền truy cập, không ai mất nó. **Lệnh cấp quyền đó
phải được migration của chính SP3c-2 chạy lại, không được coi là đã xong** (sửa 2026-09-23):
migration `0148` đã chạy nó một lần, trong quá khứ, nên mọi role tạo ra từ đó tới nay giữ
`warehouse:*` mà không có `catalog_registry:*` tương ứng — `warehouseRedirect.test.tsx:140-173` là
cả một khối describe về đúng nhóm này. Cấp rồi mới xoá, trong một transaction, theo đúng thứ tự đó.
**Re-gate chứ không re-tag**: các route giữ nguyên path và giữ nguyên OpenAPI tag; chỉ chuỗi
`require()` và mô tả 403 thay đổi (§4 bước 4).
`catalog_registry:read` được thêm vào `CONTROL_PLANE_PERMS` trong cùng thay đổi loại bỏ
`warehouse:read` khỏi đó, để không user nào có thể bị khoá ngoài (locked out) trong khoảng giữa
hai việc đó.

**D4 — Test nói đúng thứ nó đã test.** Với một engine đã attach, `SHOW SCHEMAS FROM <name>` thông
qua engine đó. Không có engine nào, thì là một kiểm tra khả năng tiếp cận (reachability) TCP tới
host và port đã cấu hình — được gắn nhãn (labelled) đúng như vậy trong UI, vì một dấu tick xanh có
nghĩa "cổng (port) có phản hồi" không được phép đọc thành "credentials hoạt động". Các kiểm tra ở
mức driver sẽ đến cùng SP5.

**D5 — UI được ship trước khi bất cứ thứ gì bị gỡ bỏ.** SP3c-1 giữ nguyên mọi endpoint cũ tại chỗ;
SP3c-2 xoá chúng một khi không còn gì gọi tới chúng nữa. Một user nâng cấp chỉ lên 3c-1 có một hệ
thống hoạt động với hai cách để tiếp cận catalog; một user nâng cấp lên cả hai có một cách.

## 3. SP3c-1 — UI

- **Control Plane → Infrastructure → Catalogs.** List: name, kind, các engine đã attach, các
  workspace đã gán (assigned), và một cảnh báo khi bất kỳ attachment nào `failed` hoặc bị kẹt
  (stuck) ở `pending`.
- **Create**: chọn một kind, điền vào typed form, tuỳ chọn thêm một tab Advanced theo từng engine.
  Được xây từ các config primitive hiện có (`ConfigSection`, `FieldLabel`, secret picker,
  storage-connection picker).
- **Detail**: Configuration, Engines (attach, detach, apply status, Retry), Workspaces (assign),
  cộng thêm sáu tab warehouse hiện có khi catalog là iceberg-egret (D1).
- **`/control-plane/warehouses/*` redirect** tới catalog tương ứng. Ba nơi link tới path đó bằng
  literal string — `WorkspaceDetail.tsx`, `StorageDetail.tsx`, `Sidebar.tsx` — và di dời cùng với
  nó.
- **Engine detail → Catalogs** thay thế `CatalogForm` inline bằng một picker trên registry, hiển
  thị status của từng attachment và cung cấp Retry.
- **Create-engine wizard** attach các catalog thay vì chọn một warehouse duy nhất.
- **Permissions**: `catalog_registry:read` gate section mới, trong route guard, sidebar và
  `appAccess.ts`.
- **Backend, chỉ những gì còn thiếu**: một detach endpoint cho registry (generated client hiện nay
  chưa có detach hook; việc removal vẫn đi qua legacy engine route).

## 4. SP3c-2 — việc gỡ bỏ

Sửa 2026-09-23, sau một khảo sát cây mã tại `14de835a` (SP3a, SP3b và SP3c-1 đều đã merge). Khảo sát
đó sửa lại tiền đề của bước 1, thêm một điều kiện tiên quyết cho bước 2, và biến bước 3 thành một
quyết định chứ không phải một phép viết lại. Toàn bộ đi trong **một PR duy nhất**, giữ nguyên URL
(quyết định của người dùng, 2026-09-23); các bước dưới đây là ranh giới commit, không phải ranh giới
release.

**1. `engine_mgmt.engine_catalogs` — và boot backfill, cùng nhau.** Bảng này có **bốn** reader
production, không phải một: `egret-policy/src/store.rs:266-284`, hai câu quét của backfill
(`backfill.rs:122-127`, `:193-200`), và `unmigrated_legacy_catalog_count`
(`catalog-registry/src/store.rs:951-962`) — khi count khác 0 thì mọi apply Trino trả 409
(`engines.rs:641-653`). `run_backfill` chạy vô điều kiện ở mỗi lần boot
(`egret-bin/src/main.rs:1263`), nên không thể drop bảng khi backfill còn sống.

Quyết định (người dùng, 2026-09-23): **gỡ cả hai, và tuyên bố SP3a/3b/3c-1 là một hop nâng cấp bắt
buộc.** Sau SP3c-2 không còn mã nào migrate được một DB trước-SP3a, nên migration phải **báo lỗi to
thay vì mất dữ liệu** — nó raise nếu `engine_mgmt.engine_catalogs` còn một dòng mà
`registry.legacy_engine_catalogs` chưa map, và nêu rõ hop cần đi qua. Operator gặp lỗi đó thì nâng
cấp qua SP3c-1 và boot một lần.

Điều này cũng giải quyết luôn reader của policy mà không cần thêm gì vào registry. Nhánh `UNION`
legacy của nó tồn tại cho hai nhóm (`egret-policy/src/store.rs:220-232`): engine có backfill thất
bại, và cửa sổ đổi tên (rename window). Nhóm thứ nhất biến mất cùng lúc backfill biến mất. Nhóm thứ
hai **vốn đã lỗi thời** — `registry.catalogs.name` là `UNIQUE` và bất biến (`0147:11-12`;
`CreateRegistryCatalogBody.name` ghi rõ "immutable"; `UpdateRegistryCatalogBody` không mang name;
`RegistryStore::update_catalog` nói "kind and name never change"), và người đổi tên duy nhất còn lại
chính là pass thêm hậu tố của backfill. Nên cả hai nhánh legacy — `tag_assignments_for_service` và
`bump_bound_tag_versions` (`tag_version.rs:61-64`) — xoá thẳng. Chết theo: `legacy.rs` (664 dòng, mà
consumer cuối cùng là backfill), `map_legacy_row_on`, `registry.legacy_engine_catalogs`, apply guard,
năm dead store method, và mười một file test DB.

**2. Các endpoint legacy `/engines/{id}/catalogs*` — sau khi `configError` di dời.** Năm trong sáu
route đã không còn caller nào; chỉ `GET` còn, và chỉ với tư cách người mang `configError`, nên thứ tự
đã nêu là đúng. Hai điểm sửa:

- **Phép di dời phải degrade, không được propagate.** `to_dto` (`catalogs.rs:237-269`) gán `config`
  từ cột thô và không parse gì cả. Thêm một lời gọi `row.def()` cho mỗi dòng khiến một dòng mà
  validator đã siết từ chối có thể làm hỏng **cả danh sách** — đúng cái defect mà review N1 của
  SP3c-1 đã sửa. Ghi `Err` vào field `config_error` mới; tuyệt đối không `?`.
- **Một carve-out mất kênh báo.** `catalog_apply.rs:1265-1272` cố ý *không* ghi
  `apply_status = failed` cho `ApplyError::Build`, với lý do đã viết rằng nguyên nhân "được báo độc
  lập dưới dạng `configError` trên `GET /engines/{id}/catalogs`" — một trong những route sắp bị xoá.
  Di dời field trước thì kênh đó còn; xoá trước thì một lỗi build Trino vô hình trên **cả hai** mặt.
  Điều này không có trong thiết kế và nay là một phần của điều kiện tiên quyết.

Xoá các route này cũng khai tử `CatalogTestDto` với hình dạng `{ ok, message }` không có discriminant
— đó chính là thứ làm D4 đúng ở mọi nơi chứ không chỉ trên một route. Lưu ý
`POST /engines/{id}/catalogs/{cid}` và `…/apply` nằm dưới cùng tiền tố URL nhưng **thuộc registry và
ở lại**.

**Một hệ quả phải nói rõ, vì đây là mất năng lực.** Trước SP3c-2, một operator chỉ có `engine:read`
vẫn thấy được tên các catalog của engine trên tab Catalogs, vì `GET /engines/{id}/catalogs` gác bằng
`engine:read`. Danh sách registry gác bằng `catalog_registry:read`, nên sau bước này operator đó thấy
tab, thấy một cảnh báo nêu đúng tên quyền còn thiếu, và không thấy tên catalog nào. Đó là cái giá của
một họ permission duy nhất (D3) và nó được chấp nhận — nhưng tab **tuyệt đối không được** trông như
rỗng, vì một bảng rỗng đọc thành "engine này không có catalog nào". Cảnh báo nói đúng câu đó.

**3. `engines.warehouse_id` — và "primary" từ nay nghĩa là gì.** Một writer (`store.rs:203-227`),
mười một reader production; bốn trong số đó chết theo bước 1. `EngineDto.warehouse_id` rời khỏi dây,
việc đó xoá hai dòng echo ở frontend (`useEngineConfigForm.ts:1291`,
`useStarRocksConfigForm.ts:506`) mà server vốn đã bỏ qua khi `PUT`.

Hai reader cần một câu trả lời chứ không phải một phép viết lại. StarRocks suy ra builtin storage
volume từ warehouse (`engines.rs:626-635`, `:4977-4983`), và `ensure_engine_role` / `create_client`
seed Ranger role cùng Catalog Client của engine từ nó (`engines.rs:2899-2905`, `:3073`, `:3800`). Một
primary kiểu `jdbc` có `warehouse_id IS NULL` và không trả lời được cả hai.

Quyết định (người dùng, 2026-09-23): **attachment primary bắt buộc trỏ tới một catalog có
warehouse.** Backend từ chối `is_primary` trên một catalog không có, và nói rõ lý do. Đó là hành vi
hôm nay được giữ nguyên, nhờ vậy SP3c-2 vẫn là một PR gỡ bỏ; cho một catalog không-warehouse dẫn đầu
là một tính năng, và một PR xoá chừng 2 500 dòng là nơi tệ nhất để đưa vào đúng cái thứ sẽ làm gãy
một lần provision thật. Việc đó thành hạng mục riêng sau SP4.

Hai hệ quả phải ghi lại, vì chưa ai ghi:

- `EngineBinding.warehouse_id` (`egret-bin/src/main.rs:286`) nuôi autocomplete của trình soạn Ranger
  policy (`egret-policy/src/admin/mod.rs:372-396`). Một engine giờ có *N* warehouse — mỗi catalog
  egret đã attach một cái — nên nó thành `warehouse_ids: Vec<Uuid>`, và consumer vốn đã dedupe thành
  một `Vec`. Điều này **nới rộng** picker: một engine attach ba catalog egret sẽ autocomplete schema
  của cả ba warehouse. Đó là câu trả lời đúng, và nó là một thay đổi hành vi.
- Partial unique index của migration `0150` bảo đảm **nhiều nhất một** primary, không phải đúng một.
  Bỏ `engines.warehouse_id` là bỏ mỏ neo `NOT NULL` cuối cùng cho mệnh đề "một engine có primary",
  để lại `resolve_primary_catalog` là điểm cưỡng chế duy nhất.

**4. `warehouse:*` — re-gate chứ không re-tag.** Ba mươi vị trí `require()`: 27 trong
`catalog-engine/src/warehouse_admin/mod.rs`, 3 trong `egret-bi/src/catalog_tags.rs`. Migration chạy
lại lệnh cấp quyền của D3 rồi mới xoá bốn key, trong một transaction, và bốn mục `perm!` rời
`egret-core/src/rbac.rs:82-104`.

URL không di dời (quyết định của người dùng, 2026-09-23): `/control-plane/warehouses/{id}/…` giữ
nguyên path **và giữ nguyên OpenAPI tag**. Chỉ chuỗi `require()` và mô tả 403 đổi. Re-tag sẽ đổi tên
chừng bốn mươi hàm generated rải khắp `api/controlPlane.ts:209-480` và làm gãy ba spec e2e cùng các
URL viết tay trong `api/catalogTags.ts`, đổi lại không gì người dùng thấy được. Path parameter vẫn là
một warehouse id, thứ frontend vẫn đang giữ dưới dạng `RegistryCatalogDto.warehouseId`.

**5. Khu vực UI Warehouses.** Hai cái bẫy mà grep `/control-plane/warehouses` không tìm ra:

- `CatalogCreate.tsx:28` import `warehousesQueryKey` **từ `"../Warehouses"`**, không phải từ
  `"../queryKeys"` nơi nó được định nghĩa. Xoá trang đó là gãy biên dịch cho tới khi import này dời.
- `AddWarehouseDialog` (`Warehouses.tsx:258-401`) là UI tạo warehouse **duy nhất** trong cây mã.
  Không được xoá khu vực này cho tới khi `CatalogCreate` tạo được một catalog egret-warehouse trọn
  vẹn — kiểm trong trình duyệt, không suy đoán.

`WarehouseRedirect` **ở lại**, được re-gate, như một route tương thích không niêm yết: nó vốn đã trả
lời phép tra warehouse-id → catalog-id mà nếu không thì `WorkspaceDetail.tsx:409` và
`StorageDetail.tsx:546` mỗi bên phải tự mọc ra, và nó giữ cho một URL đã bookmark còn chạy. Link
`?workspace=` ở `WorkspaceDetail.tsx:377` trỏ lại `Catalogs.tsx`, nơi đã hỗ trợ bộ lọc đó phía
server. Route list, mục sidebar, `Warehouses.tsx` và hai test của nó thì đi.

**Thứ tự và rủi ro.** Bước 1, 2, 4 và 5 là cơ học một khi điều kiện tiên quyết của chúng thoả. Bước 3
thì không: nó đi thành commit riêng với kiểm sống riêng, và cổng cuối của PR **provision thật cả một
engine Trino lẫn một engine StarRocks trên kind**, vì phép suy ra storage và phép seed Ranger role là
chỗ mà một sai lầm trở thành một cluster hỏng chứ không phải một test đỏ.

## 5. Testing

- Vitest cho các trang mới, gồm các tab có điều kiện theo kind, attach picker, multi-select của
  wizard, và một redirect test cho mỗi path đã di dời.
- Một permission test rằng một user chỉ giữ `catalog_registry:read` vẫn có thể vào Control Plane
  và thấy Catalogs — chính cái lock-out mà thiết kế này tồn tại để tránh.
- Một migration test rằng mọi role đang giữ `warehouse:*` kết thúc với `catalog_registry:*` tương
  ứng và không hơn — seed bằng một role tạo **sau** `0148`, vì đó chính là nhóm mà lần chạy lại tồn
  tại để phục vụ (§4 bước 4). Hôm nay chưa có test nào như vậy.
- Một migration test rằng SP3c-2 **từ chối** một database còn dòng `engine_mgmt.engine_catalogs`
  chưa map, thay vì xoá nó đi (§4 bước 1). Một migration lặng lẽ thành công trên dữ liệu nó không
  mang theo được chính là kiểu hỏng mà guard này tồn tại để làm cho bất khả.
- Một test rằng `GET /catalogs` vẫn liệt kê đủ mọi dòng khi config đã lưu của một dòng không parse
  được nữa, và báo `configError` của đúng dòng đó — nhà mới của
  `one_unparseable_catalog_does_not_take_an_engines_whole_list_with_it`, test này di dời chứ không
  chết.
- Một backend test rằng attach `is_primary` lên một catalog không có warehouse bị từ chối, và nói rõ
  lý do.
- Live trên kind, báo cáo là thủ công và **provision cả hai loại engine**: một engine Trino và một
  engine StarRocks, mỗi cái provision sau khi cột đã biến mất, mỗi cái đạt `HEALTHY`, với storage
  volume của StarRocks và Ranger role của engine đều được kiểm chứng là suy từ catalog của
  attachment primary chứ không phải từ cột đã bị bỏ.
- `provisioning-ui-kind.spec.ts` **đã stale sẵn** — nó kỳ vọng một dialog mà giờ đã trở thành một
  full-page wizard — nên SP3c-1 viết lại nó thay vì kế thừa một baseline đã hỏng.
- Live trên kind, báo cáo là thủ công (manual): tạo một catalog trong UI, attach nó vào một engine,
  theo dõi status ổn định (settle), gán nó cho một workspace, và xác nhận workspace thấy được nó.

## 6. Ngoài phạm vi

Việc sinh Ranger policy theo từng catalog và mô hình permission của egret (SP4). Các engine được
egret-enforce (SP5). Việc chọn notebook theo assignment, thứ cần per-workspace notebook role của
SP4. Endpoint `SetNotebookWarehouse` có vẻ như không có UI caller nào cả; điều đó được ghi chú lại
cho SP4 thay vì bị xoá ở đây dựa trên một phỏng đoán.
