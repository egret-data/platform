# Phiên bản engine: egret pin gì và cách chuyển một engine

> Bản tham khảo tiếng Việt. Bản gốc (canonical) là [docs/en/guides/engine-versions.md](../../en/guides/engine-versions.md); khi hai bản khác nhau, bản tiếng Anh thắng. Thông báo lỗi và tên trường API giữ nguyên tiếng Anh.

egret đọc phiên bản Ranger plugin của mỗi engine được provision từ đúng image của engine, không bao giờ từ loại engine. Cùng một tag StarRocks có thể mang plugin khác nhau vào những ngày khác nhau, nên đoán theo loại sẽ đánh giá quyền truy cập bằng một plugin chưa ai đo.

Control plane hiện kết quả cho mọi engine: image nó chạy, phiên bản Ranger plugin mà image đó mang, và bộ đánh giá của egret có được mô hình hoá cho cặp (loại, phiên bản) đó không. Khi không, trang engine hiện một cảnh báo nói cách sửa.

## egret pin gì

Ba image dưới đây là những image egret đã đo: jar plugin được đọc ra từ chính mỗi image. Mặc định cho engine mới được pin theo digest.

| Engine | Image | Digest | Ranger plugin |
|---|---|---|---|
| Trino | `trinodb/trino:483` | `sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534` | 2.8.0 |
| StarRocks | `starrocks/fe-ubuntu:4.1.4` | `sha256:0b9099483e52ee66f9ee0244799bd32f3ccb491492857c59ada622ceb15b914b` | 2.8.0 |
| StarRocks | `starrocks/fe-ubuntu:4.1.6` | `sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31` | 2.9.0 |

Engine mới mặc định là Trino 483 và StarRocks 4.1.6 (FE và CN, mỗi cái pin theo digest). Các cặp (loại, phiên bản plugin) được mô hình hoá là Trino 2.8.0, StarRocks 2.8.0 và StarRocks 2.9.0. `GET /api/control-plane/engines/modelled-ranger-plugins` liệt kê chúng, và form đăng ký engine external chỉ đưa ra các cặp của loại đã chọn, cùng "Other…".

Tag có digest chỉ khớp nếu digest là digest đã đo. Tag không có digest khớp với tag đã đo.

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
