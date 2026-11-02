# Phiên bản engine: egret pin gì và cách chuyển một engine

> Bản tham khảo tiếng Việt. Bản gốc (canonical) là [docs/en/guides/engine-versions.md](../../en/guides/engine-versions.md); khi hai bản khác nhau, bản tiếng Anh thắng. Thông báo lỗi và tên trường API giữ nguyên tiếng Anh.

egret đọc phiên bản Ranger plugin của mỗi engine được provision từ đúng image của engine, không bao giờ từ loại engine. Cùng một tag StarRocks có thể mang plugin khác nhau vào những ngày khác nhau, nên đoán theo loại sẽ đánh giá quyền truy cập bằng một plugin chưa ai đo.

Control plane hiện kết quả cho mọi engine: image nó chạy, phiên bản Ranger plugin mà image đó mang, và bộ đánh giá của egret có được mô hình hoá cho cặp (loại, phiên bản) đó không. Khi không, trang engine hiện một cảnh báo nói cách sửa.

## egret pin gì

Bốn image dưới đây là những image egret đã đo: jar plugin được đọc ra từ chính mỗi image. Mặc định cho engine mới được pin theo digest.

| Engine | Image | Digest | Ranger plugin |
|---|---|---|---|
| Trino | `trinodb/trino:483` | `sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534` | 2.8.0 |
| StarRocks | `starrocks/fe-ubuntu:4.1.4` | `sha256:0b9099483e52ee66f9ee0244799bd32f3ccb491492857c59ada622ceb15b914b` | 2.8.0 |
| StarRocks | `starrocks/fe-ubuntu:4.1.6` | `sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31` | 2.9.0 |
| StarRocks | `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.1` (bản 4.1.7 egret đã vá, patch 0001-0002; đã thay) | `sha256:da948dad19e98a5a43b0073ec48f87c9e7429246ce4581a000c0955c742af250` | 2.9.0 |
| StarRocks | `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.3` (bản 4.1.7 egret đã vá, patch 0001-0007; mặc định) | `sha256:5c74c820f9b741bc8054247f47f3d54dd9e526710b6fc2b5a7873cae6bb2d2b0` | 2.9.0 |

Engine mới mặc định là Trino 483 và StarRocks 4.1.7: FE là image egret đã vá, CN là image chính thức `starrocks/cn-ubuntu:4.1.7` (mỗi cái pin theo digest). Xem [Image StarRocks egret đã vá](#image-starrocks-egret-đã-vá). Các cặp (loại, phiên bản plugin) được mô hình hoá là Trino 2.8.0, StarRocks 2.8.0 và StarRocks 2.9.0. `GET /api/control-plane/engines/modelled-ranger-plugins` liệt kê chúng, và form đăng ký engine external chỉ đưa ra các cặp của loại đã chọn, cùng "Other…".

Tag có digest chỉ khớp nếu digest là digest đã đo. Tag không có digest khớp với tag đã đo.


## Image StarRocks egret đã vá

Image StarRocks chính thức có những lỗ mà không policy nào egret biên dịch đóng được. Mỗi lỗ đã được đo trên 4.1.7 với một user bị Ranger áp mask cột và row filter, và một user không có quyền nào:

- **Bảng metadata của Iceberg không được kiểm tra.** StarRocks trả `t$files`, `t$partitions`, `t$manifests`, `t$snapshots`, `t$history`, `t$refs`, `t$metadata_log_entries` và các bảng metadata khác của một bảng Iceberg mà không hỏi Ranger. Ai đăng nhập được cũng đọc được, với mọi bảng, số dòng, giá trị partition, đường dẫn file dữ liệu và giá trị nhỏ nhất, lớn nhất của từng cột, bất kể grant, mask, row filter và classification nói gì.
- **Prepared statement bỏ qua mask và row filter.** Câu lệnh chạy qua `PREPARE`/`EXECUTE` hoặc giao thức nhị phân MySQL (`COM_STMT_PREPARE`/`COM_STMT_EXECUTE`) không bị áp mask hay row filter. Connector StarRocks của egret chạy endpoint Data API có tham số theo cách này, nên trên image chính thức endpoint đó trả mọi dòng và cột ở dạng rõ (upstream issue [#78176](https://github.com/StarRocks/starrocks/issues/78176)). `PIVOT` cũng bỏ qua như vậy.
- **HTTP SQL API và Arrow Flight SQL bỏ qua mask và row filter.** Câu truy vấn gửi tới cổng HTTP của FE (`/api/v1/catalogs/<catalog>/databases/<db>/sql`, cổng 8030, mà Ingress của StarRocks đưa ra ngoài) hoặc tới Arrow Flight SQL trả mọi dòng và cột ở dạng rõ (upstream [#78175](https://github.com/StarRocks/starrocks/issues/78175)).
- **`FILES()` không có quyền riêng.** User nào cũng đọc và ghi object storage qua nó, vượt mọi grant, mask và row filter, và với `aws.s3.use_instance_profile` thì dùng chính danh tính cloud của BE (upstream [#80233](https://github.com/StarRocks/starrocks/issues/80233)).
- **Materialized view lưu dữ liệu vượt policy của người tạo.** Lần refresh đọc bảng gốc không áp mask và row filter của người tạo, nên một user bị mask và lọc dòng mà được tạo MV sẽ đọc được cả bảng qua nó.
- **`SELECT *` dưới mask hoặc row filter trả thêm `_file` và `_pos`** trên bảng Iceberg (KI-337), và **`SHOW CREATE CATALOG` không cần quyền**: user nào cũng đọc được endpoint của mọi catalog và đầu, cuối key của nó.

`ghcr.io/egret-data/egret-starrocks-fe` đóng tất cả các lỗ này từ `4.1.7-egret.3` (`4.1.7-egret.1` chỉ đóng hai lỗ đầu). Nó là image `starrocks/fe-ubuntu` chính thức cùng bản, với hai jar FE build lại từ tag của bản đó cộng các patch của egret; ngoài ra không khác gì, và nó mang jar plugin Ranger chính thức. Trên image này, `FILES()` cần quyền OPERATE trên SYSTEM, và tạo materialized view trên một bảng mà người tạo đang bị mask hoặc row filter sẽ bị từ chối. Mã nguồn, các patch, những giới hạn nó không thay đổi và bài test end-to-end bảo mật mà mọi image phải qua trước khi publish nằm ở [github.com/egret-data/egret-starrocks](https://github.com/egret-data/egret-starrocks). Compute node dùng image `starrocks/cn-ubuntu` chính thức cùng bản: không patch nào chạm tới chúng.

Trang engine hiện cảnh báo về các lỗ này cho một engine StarRocks đã gắn profile, trừ khi image FE của nó là `egret-starrocks-fe` `4.1.7-egret.3` trở lên. Engine external luôn hiện cảnh báo, vì egret không biết nó chạy image nào.

Để chuyển một engine StarRocks đang có sang image đã vá, cập nhật nó như trong [Cách pin](#cách-pin) với `feImage` `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.3@sha256:5c74c820f9b741bc8054247f47f3d54dd9e526710b6fc2b5a7873cae6bb2d2b0` và `cnImage` `starrocks/cn-ubuntu:4.1.7@sha256:39f7320127c9924abb1f1ce0ba0583ab11989cbe5bd1bb6274ada6f89ba5740d`. Từ 4.1.4 hoặc 4.1.6 đây là nâng cấp (lên 4.1.7), không bao giờ là hạ cấp; engine đang ở `4.1.7-egret.1` chuyển theo cách y hệt.

## Ranger plugin 2.8.0 là mức sàn (bỏ Trino 470)

egret hỗ trợ Ranger plugin từ 2.8.0 trở lên. Plugin 2.5.0, đi kèm `trinodb/trino:470`, không gửi được credential riêng của engine mà bề mặt Ranger của egret đòi, nên không còn được hỗ trợ. Đã đo với jar thật: khi đặt `policy.rest.client.username` và `.password`, 2.8.0 và 2.9.0 gửi `Authorization: Basic …` tới đường tải secure, còn 2.5.0 bỏ qua cả hai property và không gửi header nào ([số đo](../../en/plans/2026-10-20-ranger-surface-auth-measurements.md), mục "Floor"). Credential và bề mặt được mô tả trong [ranger-surface.md](ranger-surface.md).

- Tạo engine trên `trinodb/trino:470`, hoặc cập nhật một engine sang nó, bị từ chối với 400.
- Đăng ký engine external khai báo plugin cũ hơn 2.8.0 bị từ chối với 400 ("Ranger plugin 2.5.0 is not supported: egret requires 2.8.0 or later").
- Engine đang chạy Trino 470 vẫn đọc được nhưng bị báo là không được mô hình hoá và không được hỗ trợ; nó không được migrate. Plugin của nó không xác thực được, nên nhận 401 ở mọi lần poll: nó tiếp tục cưỡng chế policy đã cache, không nhận thay đổi nào, và từ chối mọi thứ sau một lần restart không có cache. Nâng nó bằng `PUT /api/control-plane/engines/{id}` với `imageVersion: "483"`, hoặc xoá nó.

Vì vậy các cặp (engine, plugin) được hỗ trợ là Trino 2.8.0, StarRocks 2.8.0 và StarRocks 2.9.0, và các gate Ranger (`just test-ranger-gates`) chỉ chạy jar thật 2.8.0 và 2.9.0.

Tên engine StarRocks tối đa 48 ký tự (400 khi tạo và ở mọi lần apply của engine StarRocks đã gắn profile), vì operator đặt tên mỗi volume của FE bằng tên Secret hay ConfigMap nó mount cộng 4 ký tự hash, và tên volume tối đa 63 ký tự.

## StarRocks 4.1.6 thay đổi gì

StarRocks 4.1.6 mang Ranger plugin 2.9.0 và cưỡng chế quyền trên view mà 4.1.4 không cưỡng chế. Lần đo ([`docs/en/plans/2026-10-13-view-clearance-measurement.md`](../../en/plans/2026-10-13-view-clearance-measurement.md), vòng 2, Câu hỏi B) thấy, trên cùng object và cùng user:

> F-1 and F-2 are fixed in StarRocks 4.1.6 (`ranger-plugins-common` 2.9.0), by upstream commit `e0d97ad2659` / backport `2a36c2d0715`, absent from 4.1.4.

Trên thực tế, ở 4.1.6:

- User không có grant không còn đọc được plain view. Ở 4.1.4 user đó đọc được dòng thật (F-1); ở 4.1.6 lần đọc bị từ chối với `ERROR 5204 ... SELECT ... VIEW <name>`.
- `SECURITY INVOKER` giờ được cưỡng chế. Ở 4.1.4 mệnh đề này được chấp nhận, lưu và in ra nhưng không được cưỡng chế dưới Ranger (F-2); ở 4.1.6 việc kiểm bảng gốc chạy với tư cách người đọc.

**Hãy chờ đợi lời từ chối ở những chỗ view từng được đọc mà không có grant.** Sau khi chuyển một engine StarRocks từ 4.1.4 sang 4.1.6, người đọc nào dựa vào một view mà không giữ grant trên nó sẽ bắt đầu nhận `ERROR 5204`. Hãy grant view, hoặc các cột gốc với view `INVOKER`, cho những người đọc nên có nó.

Không đổi với 4.1.6, theo cùng lần đo: plain view vẫn là ranh giới truy cập và không xét deny trên bảng gốc của nó, và materialized view do người đọc dựng vẫn mang dữ liệu vượt qua mask hay deny.

## Engine trên tag trôi

Engine có image là tag trôi (`starrocks/fe-ubuntu:4.1-latest`, `latest`, một tag egret chưa đo) bị báo là **không được mô hình hoá**. egret không biết nó chạy plugin nào: `starrocks/fe-ubuntu:4.1-latest` mang plugin 2.8.0 ngày 2026-09-28 và 2.9.0 từ 2026-10-01.

Với engine như vậy:

- `explain` từ chối trả lời cho profile của nó, và migration vào profile của nó bị từ chối, vì không cái nào chứng minh được trên một phiên bản plugin chưa ai đo.
- Việc cưỡng chế không đổi. Policy vẫn được biên dịch và tải xuống plugin của engine, và nó vẫn cưỡng chế chúng.
- Trang engine hiện lý do và cách sửa trong một cảnh báo, và API trả nó ở `rangerPluginVersionNote`.

Engine external được đánh giá theo phiên bản operator của nó khai báo, và không được mô hình hoá khi cặp đó không có trong danh sách trên.

## Cách pin

Gửi `PUT /api/control-plane/engines/{id}` với image đầy đủ. Body là body tạo engine: `name`, `coordinatorReplicas`, `workerReplicas`, `cpuMin`, `cpuMax` và `memory` là bắt buộc, mọi thứ khác có thể bỏ. Image bị bỏ thì giữ image engine đang chạy, nên một lần đổi kích thước không bao giờ chuyển engine sang mặc định mới hơn.

Trino, `imageVersion` là tag trần và tuỳ chọn kèm digest:

```json
{
  "name": "analytics-trino",
  "coordinatorReplicas": 1,
  "workerReplicas": 2,
  "cpuMin": "1",
  "cpuMax": "2",
  "memory": "4Gi",
  "imageVersion": "483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534"
}
```

egret lưu cái này ở `spec.imageVersion` và hiện là `trinodb/trino:483@sha256:...`.

**Image override của Trino thắng `imageVersion`.** Khi config theo vai trò của engine đặt `trinoConfig.image` (trường "Image" trên trang config của engine), provisioner chạy image đó và bỏ qua `imageVersion`. egret đọc cùng trường đó: trang engine, `image` của API và việc phân giải plugin đều dùng override như đã viết, và bảng image chính xác ở trên quyết định nó nghĩa là gì. Vì vậy:

- `trinoConfig.image: "trinodb/trino:483"` (hoặc kèm digest đã đo) phân giải ra plugin 2.8.0;
- `trinoConfig.image: "trinodb/trino:483@sha256:<digest khác bất kỳ>"` không được mô hình hoá;
- image tuỳ biến hay mirror (`registry.corp/trino-custom:x`) không được mô hình hoá, và ghi chú nêu tên nó, kể cả khi `imageVersion` là 483. egret không biết image dựng lại mang plugin nào.

Override rỗng tính như không có override, ở cả provisioner lẫn báo cáo. Để pin một engine có override, hoặc đặt override thành một image đã đo, hoặc xoá nó và pin `imageVersion`.

StarRocks, object `starrocks` của body nhận `feImage` và `cnImage`; egret lưu chúng ở `spec.starrocksConfig.feImage` và `spec.starrocksConfig.cnImage`. Việc phân giải plugin đọc `feImage`:

```json
{
  "name": "analytics-sr",
  "coordinatorReplicas": 1,
  "workerReplicas": 1,
  "cpuMin": "1",
  "cpuMax": "2",
  "memory": "4Gi",
  "starrocks": {
    "feImage": "starrocks/fe-ubuntu:4.1.6@sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31",
    "cnImage": "starrocks/cn-ubuntu:4.1.6@sha256:bf0f5ca7fd9fc4e568d4d0e593e362f229b376976b513a5a549b1b922d9ec23b"
  }
}
```

**Đổi image thì restart pod của engine.** Không có bước restart riêng. Route update lưu spec mới, dựng lại request provision từ nó và apply lên cluster qua provisioner (cùng lời gọi như mọi lần update). Trino không có hot reload, nên provisioner roll pod sang image mới; với StarRocks, operator reconcile workload FE và CN sang image mới. Engine sẽ không dùng được trong lúc pod roll. Nếu cluster từ chối lần apply, egret đặt lại spec cũ và request thất bại kèm lý do của cluster. Sau lần apply, kết nối của mọi workspace được gán được materialise lại.

Trang engine và `GET /api/control-plane/engines/{id}` khi đó báo `image` mới, phiên bản plugin nó mang và `rangerPluginVersionModelled: true`.

Engine external không bao giờ được cập nhật theo cách này: egret không chạy cluster của nó. Khai báo phiên bản plugin bạn đã cài bằng `PATCH /api/control-plane/engines/{id}/external`.

### Pin một engine StarRocks từng chạy `4.1-latest`

Migration 0185 ghi `starrocks/fe-ubuntu:4.1-latest` và `starrocks/cn-ubuntu:4.1-latest` vào mọi engine StarRocks được provision mà không nêu image, vì đó là tag trôi các engine đó đã chạy. egret báo engine như vậy là không được mô hình hoá: **`explain` và migration cho profile của nó bị từ chối cho tới khi bạn pin nó.** Trong lúc đó việc cưỡng chế không đổi.

Đừng chọn bản pin từ bảng theo thói quen. `4.1-latest` đã phân giải thành 4.1.6 từ 2026-10-01, nên engine có pod kéo image vào hoặc sau ngày đó đã chạy 4.1.6, và pin nó về 4.1.4 sẽ là **hạ cấp**. StarRocks không hỗ trợ hạ cấp metadata của FE: một FE đã ghi metadata 4.1.6 có thể không khởi động được trên 4.1.4, hoặc khởi động trên metadata nó không hiểu đầy đủ.

Đọc xem pod FE thật sự chạy gì, rồi pin theo image đã biết tương ứng:

```bash
kubectl -n <namespace> get pod <engine-name>-fe-0 -o jsonpath='{..imageID}'
```

Kết quả kết thúc bằng digest đang chạy (`...@sha256:<digest>`). So nó với bảng trên:

- `sha256:d07aa750...` là 4.1.6: pin `starrocks/fe-ubuntu:4.1.6@sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31`, và image CN về `starrocks/cn-ubuntu:4.1.6@sha256:bf0f5ca7fd9fc4e568d4d0e593e362f229b376976b513a5a549b1b922d9ec23b`.
- `sha256:0b909948...` là 4.1.4: pin image FE 4.1.4. Chuyển nó sang 4.1.6 sau đó là nâng cấp, kèm các thay đổi quyền trên view nói ở trên.
- Digest khác là image egret chưa đo. Chỉ pin nó về image đã biết cùng phiên bản StarRocks nếu bạn xác nhận được phiên bản đó; nếu không, chuyển nó tiến lên 4.1.6, không bao giờ lùi.

Kiểm pod CN theo cùng cách (`<engine-name>-cn-0`) và pin image CN cùng phiên bản với FE.
