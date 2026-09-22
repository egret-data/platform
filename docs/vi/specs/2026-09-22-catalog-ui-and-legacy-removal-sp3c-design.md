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
đó cột này bị xoá. Primary là một thuộc tính của *quan hệ (relationship)* giữa một engine và một
catalog — điều mà nó vốn luôn là như vậy — lưu nó trên engine chính là thứ đã ép buộc một warehouse
cho mỗi engine.

**D3 — Một họ permission (permission family) duy nhất.** `warehouse_admin` và các endpoint
resource-tag chuyển sang `catalog_registry:*`, và một migration cấp
`catalog_registry:{read,create,update,delete}` cho mọi role đang giữ `warehouse:*` tương ứng,
trước khi các key cũ bị bỏ. Không ai được thêm quyền truy cập, không ai mất nó.
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

1. `engine_mgmt.engine_catalogs`: không còn writer production nào. Chuyển legacy read của
   `egret-policy` sang registry, sau đó drop bảng này và năm dead store method.
2. Các endpoint legacy `/engines/{id}/catalogs*`, một khi UI mới là caller duy nhất.
3. `engines.warehouse_id`, sau khi `is_primary` của D2 được backfill.
4. `warehouse:*`, sau migration cấp quyền của D3 và việc re-gate `warehouse_admin`.

Mỗi bước đều có thể revert độc lập, và mỗi bước lands kèm test riêng của nó.

## 5. Testing

- Vitest cho các trang mới, gồm các tab có điều kiện theo kind, attach picker, multi-select của
  wizard, và một redirect test cho mỗi path đã di dời.
- Một permission test rằng một user chỉ giữ `catalog_registry:read` vẫn có thể vào Control Plane
  và thấy Catalogs — chính cái lock-out mà thiết kế này tồn tại để tránh.
- Một migration test rằng mọi role đang giữ `warehouse:*` kết thúc với `catalog_registry:*` tương
  ứng và không hơn.
- `provisioning-ui-kind.spec.ts` **đã stale sẵn** — nó kỳ vọng một dialog mà giờ đã trở thành một
  full-page wizard — nên SP3c-1 viết lại nó thay vì kế thừa một baseline đã hỏng.
- Live trên kind, báo cáo là thủ công (manual): tạo một catalog trong UI, attach nó vào một engine,
  theo dõi status ổn định (settle), gán nó cho một workspace, và xác nhận workspace thấy được nó.

## 6. Ngoài phạm vi

Việc sinh Ranger policy theo từng catalog và mô hình permission của egret (SP4). Các engine được
egret-enforce (SP5). Việc chọn notebook theo assignment, thứ cần per-workspace notebook role của
SP4. Endpoint `SetNotebookWarehouse` có vẻ như không có UI caller nào cả; điều đó được ghi chú lại
cho SP4 thay vì bị xoá ở đây dựa trên một phỏng đoán.
