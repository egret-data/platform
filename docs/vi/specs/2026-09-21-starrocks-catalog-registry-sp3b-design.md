# SP3b — StarRocks trên catalog registry

**Trạng thái:** đã thiết kế; mở rộng `2026-09-19-catalog-registry-design.md` (SP3a đã merge trong #587)
**Ngày:** 2026-09-21

SP3a đưa mọi catalog vào `registry.catalogs` và dạy cho Trino đọc được nó. StarRocks bị từ chối
một cách có chủ đích (`catalog_apply::STARROCKS_NOT_YET`). SP3b gỡ bỏ sự từ chối đó.

## 1. Vì sao StarRocks không phải là Trino

| | Trino | StarRocks |
|---|---|---|
| Một catalog là | một file `.properties` trong một ConfigMap | một row trong FE metadata, được tạo bằng DDL |
| Áp dụng (applying) | ghi k8s object; operator roll lại pod | `CREATE` / `ALTER` / `DROP CATALOG` trên một FE đang sống |
| Cần cluster đang chạy | không | **có** |
| Idempotent | có — toàn bộ spec được render lại ở mỗi lần apply | **không** — statement one-shot |
| Secret | `${ENV:…}`, được resolve vào một k8s Secret | **inline trong văn bản DDL** |
| Xoá một catalog cũ (stale) | lần apply tiếp theo sẽ prune nó | không có gì prune nó; phải drop thủ công |

Bốn hệ quả dẫn dắt thiết kế này: một apply có thể bị từ chối vì cluster chưa sẵn sàng; một thay
đổi áp dụng nửa chừng (half-applied) là chuyện bình thường, không phải ngoại lệ; egret phải tự bù
trừ (compensate) cho chính các write của mình; và FE giữ credential mà egret không thể gián tiếp
hoá (indirect away) được.

## 2. Các quyết định

**D1 — Attach ghi trước, rồi hội tụ (converges) sau.** Một attach ghi row attachment và thử DDL.
Khi thất bại nó lưu `apply_status = 'failed'` kèm lỗi, và trả về 201 với status đó thay vì 502. Sự
hội tụ (convergence) đến từ bootstrap đã được tổng quát hoá (D2) và route Retry hiện có. Điều này
khác với cách apply-rồi-ghi (apply-then-write) của Trino, và khác một cách có chủ đích: một FE
StarRocks không thể tiếp cận được trong năm đến mười phút sau `create_engine`, đúng vào lúc
create-engine wizard attach các catalog. Detach vẫn apply trước: một `DROP CATALOG` thất bại thì
giữ lại row, vì một row mà egret đã quên mất còn tệ hơn một row nó sẽ retry.

**D2 — Bootstrap hội tụ mọi attachment, không chỉ primary.** `spawn_starrocks_catalog_wire` lặp
qua các attachment của engine theo thứ tự tên, ghi `apply_status` cho từng attachment thay vì chỉ
log. Nó vẫn giữ sáu checkpoint `engine_still_current` của mình. Nó trở nên tiếp cận được từ route
Retry, nên admin có cách để hội tụ một engine mà hiện nay chỉ có thể sửa bằng cách tạo lại nó.

**D3 — Create được thực hiện sau một lần read.** Trước `CREATE EXTERNAL CATALOG`, egret đọc
`SHOW CATALOGS` và, khi tên đã tồn tại, đọc `SHOW CREATE CATALOG`. Không có → create. Có và khớp
với những gì registry sẽ render → coi như đã applied. Có nhưng **khác** → từ chối, với một lỗi nêu
tên catalog và các property khác biệt, và để FE nguyên vẹn. DDL hiện tại là
`CREATE EXTERNAL CATALOG IF NOT EXISTS`, thứ âm thầm chấp nhận (adopt) bất cứ thứ gì đang có ở đó —
kể cả catalog `egret` mà Warehouse UI hiện nay bảo operator tự tạo bằng tay, và bất kỳ catalog nào
còn sót lại từ một engine tiền-SP3a. Re-apply một catalog đã sửa dùng `ALTER CATALOG … SET`, không
bao giờ dùng lại `CREATE` lần hai, vì `IF NOT EXISTS` khiến một lần rotate credential trở thành
no-op trong im lặng.

**D4 — StarRocks 4.x là mức sàn (floor).** Egret provision `starrocks/fe-ubuntu:4.1-latest`.
`can_read` gate theo đó: iceberg, hive, deltalake, hudi, elasticsearch, tpch/tpcds (render thành
`benchmark`), và jdbc cho postgresql, mysql, mariadb, oracle, sqlserver và clickhouse. Image này
overridable được, nên một engine pin vào một FE cũ hơn vẫn có thể từ chối một kind mà egret cho
phép; sự từ chối đó lộ ra như lỗi DDL trên attachment, không phải như một lỗi validation. Hiện nay
`can_read` trả về `true` cho mọi kind đã typed trên cả hai engine, điều này đơn giản là sai, và
SP3b là nơi nó thôi sai.

**D5 — Driver JDBC được pin trong config của egret, override được theo từng catalog.** StarRocks
khiến FE tải về driver được nêu tên bởi `driver_url`. Egret cung cấp sẵn một `driver_url` và
`driver_class` cho từng dialect, pin theo một version chính xác, nằm trong configuration thay vì
trong code, để một platform air-gapped có thể trỏ lại (repoint) chúng tới một mirror nội bộ.
`advanced.starrocks.driver_url` override cho một catalog. Một dialect không có URL đã pin và không
có override sẽ bị từ chối tại thời điểm validation.

**D6 — Secret được resolve tại thời điểm build DDL và không bao giờ bị log.** Không có `${ENV:…}`
cho StarRocks; plaintext đi thẳng vào statement. Do đó: văn bản DDL không bao giờ được log, kể cả
ở mức debug, và lỗi trả về từ FE được scrub trước khi chúng chạm tới cột `apply_error` hoặc một API
response. DDL catalog cũng bắt đầu dùng chứng chỉ (certificate) FE đã pin của engine, thứ mà đường
delegation đã truyền sẵn còn đường catalog thì chưa — hiện nay một credential OAuth2 đi qua đường
truyền dạng cleartext trên một engine có khả năng TLS. Bất kỳ ai có quyền admin FE vẫn có thể đọc
lại credential bằng `SHOW CREATE CATALOG`; đó là mô hình của StarRocks, và SP4 là nơi authorization
theo từng user thôi bị xem nhẹ hơn mức nó đáng được.

**D7 — Egret tự bù trừ (compensate) cho các half-write của chính mình.** Nếu DDL thành công mà row
attachment không ghi được, egret drop lại catalog đó. Comment của SP3a nói rằng lần apply tiếp
theo sẽ prune nó; điều đó đúng với Trino và sai với StarRocks. Detach coi "unknown catalog" là
thành công, nên một row luôn có thể được dọn sạch, và vẫn tiếp tục từ chối detach catalog chính
(primary) của engine.

**D8 — DDL runner trở thành injectable.** `run_starrocks_ddl` được tiếp cận qua một trait mà test
có thể fake. Hiện nay phần bootstrap sau khi FE-ready hoàn toàn không test được — nó dial vào một
MySQL port thật — nên năm trên sáu guard checkpoint của nó được pin bởi một test grep vào nội dung
hàm (function body). SP3b thay thế điều đó bằng test thật.

## 3. Những gì SP3b không làm

Catalogs UI, redirect của Warehouses và việc gỡ bỏ `engine_catalogs`, `engines.warehouse_id` và
`warehouse:*` thuộc về SP3c. Việc sinh Ranger policy theo từng catalog thuộc SP4; SP3b chỉ giữ
nguyên bất biến (invariant) rằng một catalog có một tên duy nhất trên mọi engine. StarRocks không
có tag service và sẽ không có cho tới khi upstream starrocks#67458 được merge (lands), nên không
có gì ở đây thay đổi cách xử lý tag.

## 4. Testing

- Rendering theo từng kind và dialect, so với DDL kỳ vọng viết tay, gồm cả cách viết (spelling)
  `benchmark` và `deltalake`, và mọi driver đã pin.
- Một DDL runner giả (faked) lái qua: attach vào một FE đã sẵn sàng, attach vào một FE không tiếp
  cận được (row được ghi, status `failed`, sau đó được hội tụ bởi Retry), từ chối vì drift, rotate
  credential qua `ALTER`, detach với một catalog không xác định (unknown), và một lần drop bù trừ
  (compensating).
- Các checkpoint của bootstrap trở thành behavioural test thay vì một lần grep văn bản nguồn.
- Live trên kind: provision một engine StarRocks, attach một catalog iceberg và một catalog
  postgres, query cả hai, rotate password, re-apply, detach. Báo cáo là thủ công (manual).
