# Quản trị view: clearance bảo vệ gì qua view, và không bảo vệ gì

> Bản tham khảo tiếng Việt. Bản gốc (canonical) là [docs/en/guides/view-governance.md](../../en/guides/view-governance.md); khi hai bản khác nhau, bản tiếng Anh thắng. Các thông báo từ chối được giữ nguyên tiếng Anh vì đó là chuỗi engine in ra.

Clearance của một phân loại chặn đọc trực tiếp cột đã phân loại trên cả hai engine. Nếu không làm gì thêm, view đọc xuyên qua nó. egret đóng đường này theo cách khác nhau trên mỗi engine; tài liệu này nói rõ cái gì được bảo vệ, cái gì không, và phải làm gì khi egret từ chối.

Thiết kế: `docs/en/specs/2026-10-13-view-governance-design.md`. Phiên bản engine và cách pin: [engine-versions.md](../../en/guides/engine-versions.md).

## Được bảo vệ

### StarRocks

- **Đọc trực tiếp một cột đã phân loại trả về `NULL`, bất kể ai deny nó, miễn là truy vấn có đọc một cột khác mà người đọc được phép select.** Mỗi cột đã phân loại có một clearance mask: `MASK_NULL` cho người chưa được cấp clearance, không biến đổi cho người đã được cấp. StarRocks 4.1.6 không kiểm quyền truy cập trên cột có mask quyết định là `MASK_NULL`, từ bất kỳ policy nào, nên lần đọc trả về `NULL` cho cột đó thay vì `ERROR 5204`, và một deny `select` viết tay trên cột đó cũng không được áp dụng. Cả truy vấn vẫn bị từ chối trừ khi người đọc được phép select một cột khác mà truy vấn đọc; khi truy vấn chỉ select những cột như vậy, StarRocks kiểm cột chưa bị mask đầu tiên của bảng (hoặc `___count___` cho `count(*)`). Vì vậy người không có grant bị từ chối, qua cột khác đó. Explain hiện cột này là "not access-checked on StarRocks", kèm mask và policy đã quyết định nó, không có verdict `select`. Cột bị mask theo kiểu khác (đã đo `MASK_HASH`) được kiểm quyền như thường.
- **Khi clearance mask bị bỏ qua, deny quyết định.** egret bỏ qua clearance mask của một cột khi engine có plugin egret không mô hình hoá, các plugin được gắn fold khác nhau, hoặc một mask trên cột nêu group khác `public` hay một role. Lần recompile báo việc bỏ qua đó, và khi ấy clearance deny của cột từ chối lần đọc của người chưa được cấp.
- **Chưa đo: bảng mà mọi cột đều là `MASK_NULL` với người đọc** (bảng phân loại toàn bộ, người đọc chưa được cấp clearance và không có grant). Chưa biết khi đó StarRocks kiểm cột nào. Nếu không kiểm cột nào, người đọc như vậy biết được số dòng. Hãy coi bảng phân loại toàn bộ là lộ số dòng cho tới khi đo được.
- **View trả về `NULL`, hoặc bị từ chối.** Mask trên cột gốc áp dụng qua plain view và view lồng nhau, nên người chưa được cấp nhận `NULL`. Qua view INVOKER, StarRocks 4.1.6 từ chối người chưa được cấp (`ERROR 5204 … SELECT privilege(s) on COLUMN …`). Cả hai đều fail-closed.
- **Materialized view mới bị từ chối.** Materialized view lưu giá trị thật, và mask không đi theo dữ liệu đã lưu. Vì vậy trên mỗi profile StarRocks có cột đã phân loại, `create materialized view` bị deny trên `default_catalog`, mọi database. StarRocks chỉ tạo materialized view trong `default_catalog` và kiểm quyền trên database đích, nên deny có hiệu lực toàn engine.

### Trino

- **View trong các catalog Iceberg của egret mang phân loại của bảng gốc.** egret tự chạy catalog, nên `CREATE VIEW` hoặc `CREATE OR REPLACE VIEW` đến egret trước khi được commit. egret hỏi engine view đọc những bảng gốc nào (`EXPLAIN (TYPE IO)`), gắn tag phân loại của các bảng đó lên mọi cột của view, và giữ commit cho tới khi mọi plugin Trino đã có tag mới. Trước lúc đó view chưa tồn tại, nên không ai đọc được nó khi chưa có tag.
- **Tag đi theo bảng gốc.** Phân loại, đổi phân loại hay bỏ phân loại một cột gốc sẽ gắn lại tag cho mọi view có lineage đọc bảng đó, trong cùng một tag version.
- Lineage ở mức bảng: mọi cột của view nhận mọi phân loại của mọi bảng nó đọc. Gắn thừa là chủ ý.

## Không được bảo vệ

- **Dữ liệu người đã được cấp clearance sao chép ra** (`CREATE TABLE AS SELECT`, `INSERT … SELECT`, export). Clearance không đi theo dữ liệu.
- **View của connector khác** (memory, Hive, hay catalog không có trong registry của egret): không đến egret, nên không được gắn tag.
- **Bảng native của StarRocks.** Registry không chứa được `default_catalog`, nên bảng native không mang được phân loại.
- **Engine cũ hơn StarRocks 4.1.6.** Ở đó người không có grant trên plain view vẫn đọc được nó (lỗi của engine). Mask vẫn null cột đã phân loại, nhưng grant trên view không được kiểm. Pin engine lên 4.1.6 theo [engine-versions.md](../../en/guides/engine-versions.md).
- **Materialized view đã tồn tại.** Deny chỉ chặn cái mới. Materialized view có sẵn có thể chứa giá trị phân loại thật, và egret không drop nó. Trang phân loại liệt kê những cái tìm thấy ("Existing materialized views"); hãy tự drop hoặc dựng lại.
- **Materialized view trên Trino** không tạo được trên Iceberg REST catalog khi đo, nên nằm ngoài phạm vi.

## Trang phân loại: các view đọc phân loại này

- **Existing materialized views (StarRocks).** Mỗi engine StarRocks đang chạy được hỏi một lần, với tư cách root. Trên StarRocks 4.1.6, root là superuser dưới Ranger và thấy mọi materialized view, nên không policy nào giấu được một view khỏi danh sách này. View khớp khi bản đồ bảng gốc của nó nêu một bảng đã phân loại, hoặc, nếu không có bản đồ, khi định nghĩa của nó nêu catalog và database đã phân loại ("matched by text"). Engine egret không hỏi (external, hoặc không chạy) được ghi "not checked". Engine không kết nối được thì trang nói vậy, không bao giờ hiện "không có".
- **Trino views (audit DEFINER).** Mọi view trong catalog của egret có lineage đọc một bảng của phân loại này, hoặc một bảng mà rule mask hay row-filter chọn. Mỗi view hiện:
  - **Mode**: DEFINER hay INVOKER, đọc từ thuộc tính lưu trữ `trino.run-as-owner` (có với DEFINER, giá trị là owner). Không đọc được metadata thì mode là "unknown"; egret không bao giờ đoán DEFINER.
  - **Owner** và **owner clearance**: owner có giữ role cấp clearance của mọi phân loại mà lineage của view chạm tới không. "Clearance unknown" nghĩa là lineage chưa tính được, hoặc một clearance chạm tới chỉ nêu group.
  - **Lineage**: view có lineage missing hoặc failed vẫn được liệt kê, là "may read classified data". Chạy lineage sweep để tính.

  Policy viết tay dạng raw trong profile manual không phải là dòng `perm.rules`, nên audit không thấy.

  Owner clearance đọc theo membership role hiện tại; engine áp dụng lần compile gần nhất, nên khi một grant role hết hạn, trang ghi "not cleared" trong khi engine vẫn cho owner qua, tới lần recompile kế tiếp.

  Danh sách Trino có hạn 15 s. Không đọc hết được view trong thời gian đó, hoặc một lần liệt kê lỗi, thì trang hiện lỗi thay cho danh sách, không bao giờ hiện danh sách thiếu.

### Vì sao liệt kê view DEFINER: khuyến nghị SECURITY INVOKER

View Trino mặc định là DEFINER: nó đọc bảng gốc với quyền của owner. Có tag rồi thì view DEFINER trên dữ liệu phân loại không còn rò rỉ, nhưng còn hai hành vi gây bất ngờ:

- owner chưa được cấp clearance chặn mọi người đọc view, kể cả người đã được cấp;
- mask và row filter của bảng gốc áp dụng theo owner, không theo người đọc.

View INVOKER đọc với quyền người đọc. **Hãy tạo view với `SECURITY INVOKER`.** egret không viết lại view.

**Lỗi Trino 483: chuyển bằng `DROP VIEW` rồi `CREATE VIEW`.** `CREATE OR REPLACE VIEW … SECURITY INVOKER` không xoá thuộc tính `trino.run-as-owner`, nên view từng là DEFINER vẫn là DEFINER:

```sql
DROP VIEW ice.sales.v_orders;
CREATE VIEW ice.sales.v_orders SECURITY INVOKER AS SELECT ...;
```

## Thời gian chờ của `CREATE VIEW`

Mọi thay đổi view trên catalog có profile chứa cột đã phân loại đều qua **hold**: create, replace hay rename, dù bảng của chính view có được phân loại hay không.

- **Một hạn 90 s** (gấp ba chu kỳ poll 30 s của plugin) tính từ lúc request đến, bao gồm lock của view, `EXPLAIN` lineage (tối đa 5 s) và thời gian chờ plugin. Trino đã được đo chờ 65 s mà không retry; 90 s chưa được đo với client của Trino.
- **Proxy đứng trước `/catalog` phải đọc lâu hơn 90 s.** Helm chart của egret định tuyến `/catalog` qua một Ingress riêng với read timeout 120 s. Proxy cắt sớm hơn sẽ trả lỗi gateway cho client trong khi egret vẫn commit hoặc từ chối view.
- **View đọc dữ liệu phân loại chờ một đến hai chu kỳ poll** (30 đến 60 s). Plugin tải tag mới ở lần poll kế tiếp, và chỉ báo version mới ở lần poll sau đó; lần báo đó mới là xác nhận. View không đọc bảng phân loại nào thì không bị trễ.
- **Tối đa 8 hold cùng lúc** trên mỗi tiến trình egret. Cái thứ chín bị từ chối ngay với thông báo retry, không xếp hàng. dbt chạy hơn 8 thread sẽ gặp vài lần từ chối, retry là thành công.
- **Lock của view đang bận thì bị từ chối ngay** với "retry".
- Catalog có profile **không** có cột phân loại thì commit không bao giờ bị giữ hay từ chối; egret vẫn ghi lineage, best-effort.

### Các lần từ chối và cách xử lý

Mọi lần từ chối là Iceberg `409`, Trino in ra `egret refused the view: <reason>`.

| Thông báo | Nghĩa | Làm gì |
|---|---|---|
| `lineage could not be computed: <detail>; check that a running Trino engine egret provisions in JWT mode has the catalog attached and can plan this SQL, then retry` | Không biết được view đọc bảng nào: EXPLAIN lỗi hoặc quá hạn (`the EXPLAIN did not answer within 5 s`), hoặc không có engine phù hợp. Cũng gồm `the engine reports no input table; egret refuses views it cannot judge` (optimizer cắt hết input: `WHERE false`, `LIMIT 0`, vị từ hằng). | Kiểm tra có engine Trino egret provision, đang chạy, chế độ JWT, đã gắn catalog và plan được SQL. Với lineage rỗng, bỏ vị từ hằng. Rồi retry. |
| `engines did not confirm the classification tags within 90 s: <plugin ids>; check that the engines are running and polling egret's Ranger endpoint, then retry` | Một plugin Trino không tải tag mới kịp. Tag và lineage đã được rollback. | Kiểm tra engine được nêu đang chạy và tới được Ranger endpoint của egret, rồi retry. |
| `no Trino engine is bound to the view's catalog, so no plugin can confirm its classification tags; attach a Trino engine to the catalog, then retry` | Không có plugin nào xác nhận được tag. | Gắn một engine Trino vào catalog, rồi retry. |
| `another change to this view is being held; retry when it has finished (at most 90 s)` | Lock của view đang bị hold khác giữ. | Retry khi nó xong. |
| `egret is holding 8 classified view changes already, its maximum; retry in a minute` | Đã đạt giới hạn 8 hold. | Retry; với dbt, giảm số thread hoặc chạy lại model lỗi. |
| `egret could not write the view's classification tags within the view's deadline (90 s), because other classification changes were being written; retry` (hoặc `… in time, because …`) | Các lần ghi phân loại khác giữ lock quá hạn. Không có gì được ghi. | Retry. |
| `the classification tags cannot be assigned: <detail>; resolve the overlapping mask or row-filter policies, then retry` | Gắn tag cho view sẽ khiến hai policy mask hay row-filter quyết định khác nhau cho cùng một request. | Thu hẹp, xoá hoặc làm khớp một rule (`GET /api/control-plane/perm/overlaps` liệt kê các cặp), rồi retry. |
| `the view's namespace has more than one level, which egret cannot govern on a catalog with classified columns; use a single-level schema` | Resource Ranger chỉ có một mức schema. | Tạo view trong schema một mức. |
| `<why>: egret cannot judge this view definition on a catalog with classified columns; create the view from Trino, with Trino SQL and a single-level default schema` | Một create egret không đọc được. | Tạo view từ Trino. |
| `<why>: egret cannot judge this view version on a catalog with classified columns; change the view's definition, or DROP and CREATE it` | Một replace egret không đọc được. | Đổi định nghĩa, hoặc `DROP VIEW` rồi `CREATE VIEW`. |
| `egret could not hold the view (<what>): <error>; retry` | Lỗi nội bộ. | Retry; nếu lặp lại, xem log của egret. |

**Undrop view bị từ chối trên profile có phân loại** (`409`), vì khôi phục đi qua API quản trị của lakekeeper mà hold không thấy:

> egret refused to restore the view: its catalog's profile has classified columns, and a restored view would bypass the classification hold; re-create it with CREATE VIEW from Trino instead

Hãy tạo lại view bằng `CREATE VIEW`. Undrop bảng, hoặc undrop view trên profile không phân loại, vẫn được phép.

## Lineage sweep

Sweep tính lại lineage của mọi view và gắn lại tag:

- **mỗi 15 phút**, theo lịch;
- **theo yêu cầu**: `POST /api/control-plane/classifications/lineage-sweep` (quyền `tag:manage`). Gặp sweep đang chạy thì trả `a lineage sweep is already running; its report is not shared, retry when it ends`;
- **ngay sau một lần ghi phân loại**, giới hạn ở các view chưa có lineage tính được (missing hoặc failed) trong mọi warehouse mà profile của lần ghi chạm tới.

View có EXPLAIN lỗi vẫn giữ tag; tag không bao giờ bị gỡ khi không chắc chắn.

View phải chờ **lịch 15 phút** thay vì sweep ngay trong các trường hợp:

- **Bỏ qua vì đang hold**: lock của view đang bị một hold giữ.
- **EXPLAIN lỗi** trong lần sweep đó.
- **Chạy đua với bước kiểm tra phân loại**: hold đọc profile là chưa phân loại, EXPLAIN best-effort lỗi, và view commit sau khi sweep của lần phân loại đã liệt kê namespace.
- **Mất notification** trong lúc listener của egret kết nối lại.
- **Xếp sau một sweep toàn bộ**: sweep giới hạn chỉ bắt đầu khi sweep toàn bộ đang chạy kết thúc, mất tới 5 s mỗi view.

Vì vậy 15 phút là lâu nhất một nhánh bị cắt, hay một view commit khi chưa có cột phân loại, còn thiếu tag, **với điều kiện lần sweep kế tiếp EXPLAIN được view đó.** View có EXPLAIN lỗi liên tục thì ở trạng thái `failed` chừng nào lỗi còn; nó giữ tag đã có, và trang phân loại ghi "lineage could not be computed".

## Giới hạn đã biết trên engine JWT (đang sửa)

Lần chạy thật trên Trino 483 (Task 11) tìm ra ba lỗi nằm ngoài quản trị view. Chúng được sửa trong một gói riêng. Trong lúc chờ:

- **Giới hạn tốc độ cấp token của egret làm hỏng truy cập catalog của Trino khi tải cao.** Client Iceberg của Trino lấy một OAuth token mới cho mỗi query session, và egret mặc định cho 60 lần xin token mỗi phút. Khi đó các `CREATE VIEW` đồng thời, và lineage sweep 15 phút, lỗi ngay trong Trino với `RESTException: Unable to process (code: 429, type: slow_down)`. **Cách tạm:** tăng `EGRET_RATE_LIMIT__TOKEN_MAX_ATTEMPTS` (lần chạy thật dùng 100000).
- **Trino làm mới token Iceberg thất bại với egret.** Khoảng một giờ sau khi Trino khởi động, lần refresh OAuth2 (`grant_type=…token-exchange`) nhận `422 … missing field 'client_id'`, và Trino retry liên tục, làm nặng thêm giới hạn ở trên. **Cách tạm:** khởi động lại engine Trino; nó bắt đầu với token mới (lần chạy thật phục hồi như vậy).
- **Xoá một policy viết tay trong tag service không đến được plugin.** Lệnh xoá tăng version của tag service nhưng không tăng version của profile Trino được gắn, nên plugin giữ policy đã xoá. **Cách tạm:** ghi bất kỳ thay đổi nào vào profile được gắn (ví dụ lưu một policy bất kỳ trên đó); lần tăng version đó đẩy việc xoá đi.

## Rủi ro đã chấp nhận

- **Người chưa được cấp clearance cho mọi phân loại trên một engine StarRocks không tạo được materialized view nào ở đó**, kể cả trên dữ liệu không phân loại. Muốn cho ai dựng materialized view, cấp cho họ clearance của mọi phân loại trên engine đó, hoặc giữ catalog có phân loại ngoài các engine dùng để dựng materialized view.
- **Principal `egret-lineage` đọc được dữ liệu được quản trị.** EXPLAIN cần `select`, nên egret cấp `use` và `select` cho nó trên mọi catalog được quản trị, chỉ trên engine Trino egret provision ở chế độ JWT, và deny mọi thứ khác. Chỉ egret giữ credential, và egret chỉ gửi EXPLAIN với nó.
- **View bị từ chối khi engine báo không có bảng input** (`WHERE false`, `LIMIT 0`): egret từ chối thay vì commit view không có tag.
- **Nhánh bị optimizer cắt chỉ được bắt ở lần sweep sau**, tối đa 15 phút.
- **Một poll giả mạo có thể kết thúc hold sớm.** Endpoint tải về của plugin là ẩn danh. Poll giả chỉ kết thúc hold sớm: plugin thật vẫn áp tag mới ở lần poll kế tiếp, trong một chu kỳ 30 s. Poller giả liên tục báo version cũ thì chặn hold, khiến view phân loại bị từ chối (fail-closed). Cấu hình `auth_token` của download đóng cả hai.
