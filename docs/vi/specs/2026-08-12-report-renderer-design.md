# Trình kết xuất báo cáo — thiết kế

> Bản tiếng Anh là bản chuẩn: `docs/en/specs/2026-08-12-report-renderer-design.md`.
> Thiết kế cha: `docs/en/specs/2026-08-11-alerts-and-reports-design.md` (§5.3 đã chốt hợp đồng và
> phạm vi bảo mật của hệ thống con này). Kế hoạch: P4 trong
> `docs/en/plans/2026-08-11-alerts-and-reports-plan.md`.

**Mục tiêu:** gửi một chart hoặc dashboard theo lịch, dưới dạng ảnh, PDF, CSV hoặc văn bản thuần.

---

## 1. Vì sao phải cần trình duyệt headless

Chart của egret là ECharts, do trình duyệt vẽ. Đường xuất hiện tại đã chứng minh điều đó:
`frontend/src/components/chart/export-chart.ts#downloadPng` đọc thẻ `<svg>` đã render ra khỏi DOM
rồi raster hoá. Không có đường nào ở phía server tạo ra đúng bức ảnh ấy.

Phương án thay thế — viết lại thư viện chart bằng Rust để backend tự vẽ — đã bị loại. Nó đồng nghĩa
với hai trình kết xuất phải khớp nhau đến từng pixel mãi mãi, trong khi công việc chart của repo này
đã cho thấy chỉ để **một** trình kết xuất khớp Metabase thôi đã tốn bao nhiêu công.

Vậy nên: một trình duyệt thật, headless, chạy trong pod. Đây là hạng mục lớn và rủi ro nhất của
chương trình Alerts & Reports, và đó chính là lý do nó được xếp cuối.

---

## 2. Các quyết định đã chốt

- **Cả bốn định dạng trong một pha.** `png`, `pdf`, `csv`, `text` ra cùng nhau, dù chỉ hai định dạng
  đầu cần tới trình kết xuất.
- **Kết xuất lỗi là một lần chạy `error`**, được ghi kèm lý do và thử lại theo đúng nhịp lịch của
  chính nó — cùng bộ từ vựng mà một lần engine từ chối đang dùng. Không cơ chế retry riêng, không
  âm thầm hạ cấp xuống email chỉ có chữ.
- **Trình kết xuất là thành phần bắt buộc.** Helm chart luôn cài nó; chỉ có một hình trạng triển
  khai được hỗ trợ và không có trạng thái nửa vời nào phải giải thích.

---

## 3. Kiến trúc

Bốn mảnh, mỗi mảnh kiểm thử được độc lập:

| Mảnh | Ở đâu | Trách nhiệm |
|---|---|---|
| Render token | `egret-automation` | Cấp, đổi-một-lần, hết hạn |
| Endpoint payload | `egret-automation` | Mọi thứ trang in cần, trong MỘT phản hồi |
| Route in | SPA | Vẽ một đối tượng, không khung giao diện; báo hiệu khi xong |
| `egret-renderer` | image mới | Điều hướng, chờ tín hiệu, chụp, trả bytes |

---

## 4. Route in, và phần thực sự dễ hỏng

`/embed/print/:kind/:id?t=<token>` vẽ đúng một chart hoặc dashboard: không nav, không sidebar,
không padding — chỉ đối tượng trên nền trắng đúng kích thước yêu cầu.

**Khó không phải ở chỗ chụp, mà ở chỗ biết khi nào nên chụp.** ECharts có animation lúc mount, và
web font tải xong sau lần paint đầu tiên. Một trình kết xuất chờ cứng hai giây sẽ *thường* chụp
được chart đã vẽ xong và *thỉnh thoảng* chụp trúng lúc đang animate, cột mới cao một nửa hoặc nhãn
đang ở font dự phòng. Lỗi đó sẽ lọt ra production, vì không test nào thấy được và có khi hai mươi
lần chạy mới tái hiện một lần.

Vì vậy tín hiệu sẵn sàng phải tường minh, không dựa vào thời gian:

```js
window.__EGRET_PRINT_READY__ = true
```

chỉ được đặt sau khi **cả hai** điều kiện thoả:

1. mọi chart trên trang đã phát sự kiện `finished` của ECharts, và
2. `document.fonts.ready` đã resolve.

Trình kết xuất chờ cờ đó với một timeout cứng. **Timeout là kết xuất THẤT BẠI** — trả về lỗi và lần
chạy được ghi `error`. Tuyệt đối không được lùi về kiểu "cứ chụp những gì đang có": một chart vẽ dở
nằm trong hộp thư còn tệ hơn là không có ảnh, vì sẽ chẳng ai kiểm tra một báo cáo đã tới nơi.

`prefers-reduced-motion` được bật trong ngữ cảnh in, nên animation ngắn hoặc không có — đây là lớp
phòng thủ bổ sung cho cờ, không phải thứ thay thế nó.

---

## 5. Token, và vì sao trang chỉ gọi mạng đúng một lần

§5.3 đã chốt phạm vi: **một đối tượng, một lần dùng, ~60 giây, gắn với chủ sở hữu của lịch.** Trình
kết xuất tuyệt đối không được giữ thứ chứng thực đọc được mọi dashboard trong nền tảng — đó chính là
khác biệt giữa một dịch vụ chụp ảnh và một dịch vụ rò rỉ dữ liệu.

"Một lần dùng" buộc ta phải chọn thiết kế. Một trang dashboard bình thường gọi rất nhiều request đã
xác thực (chi tiết, rồi mỗi card một lần). Nếu token cho phép tất cả những lời gọi ấy thì "một lần
dùng" chỉ còn là một dòng chú thích chứ không phải một tính chất.

**Nên trang in chỉ gọi đúng một request.** `GET /api/render/payload`, mang token trong header
`Authorization: Bearer` chứ không đặt trong path. Nó trả về mọi thứ trang cần — cả đặc tả của đối
tượng *lẫn* dữ liệu đã chạy sẵn của từng card — được backend lắp lại qua đúng đường chạy chart hiện
có, chạy dưới danh nghĩa chủ sở hữu. Trang vẽ hoàn toàn từ payload đó và không gọi thêm gì nữa.

Các hệ quả, đều là chủ ý:

- Một lần dùng theo đúng nghĩa đen: bản ghi token bị **xoá** ngay khi đổi, trong cùng transaction
  đọc nó, nên một lần phát lại sẽ không tìm thấy gì. Không dùng cờ `used_at` mà một request đồng
  thời thứ hai có thể chen qua.
- Trình duyệt của renderer không giữ lại thứ gì tái sử dụng được. Chiếm được pod thì cũng chỉ có
  một đối tượng, trong chưa đầy một phút, mà chủ sở hữu vốn đã đọc được.
- Không thể dụ trang in đi lấy thứ khác — chẳng có endpoint thứ hai nào cho nó gọi.
- Cái giá phải trả là một phản hồi lớn hơn và backend phải lắp thứ mà trình duyệt vốn lấy từng
  phần. Đáng.

Lưu trữ: `automation.render_tokens (id, token_hash, kind, object_id, owner_id, expires_at)`. Token
là chuỗi ngẫu nhiên; chỉ hash của nó được lưu, giống mọi bearer secret khác trong repo này.

**Phân quyền là của chủ sở hữu, tại thời điểm kết xuất** — đúng quy tắc §5.2. Nếu chủ sở hữu đã mất
quyền vào dashboard thì việc cấp token thất bại và lần chạy báo lỗi.

---

## 6. Dịch vụ kết xuất

`POST /render` — chỉ trong mạng nội bộ cluster (ClusterIP, không Ingress). Trả về bytes, hoặc mã
lỗi kèm lý do (`timeout`, `navigation failed`, `http <status>`).

- **Một browser, mỗi request một page**, có giới hạn số lượng đồng thời. Ràng buộc thật sự của
  Chromium là bộ nhớ chứ không phải CPU; để số page không giới hạn chính là cách pod này bị
  OOM-kill khi một loạt lịch cùng chạy vào đầu giờ.
- **Timeout cứng cho mỗi lần kết xuất**, thấp hơn timeout của bên gọi.
- **Không có chứng thực của riêng nó.** Bí mật duy nhất nó từng thấy là token dùng-một-lần.
- **PDF** đi qua đúng trang đó bằng print-to-PDF của Chromium, nên ảnh và PDF của cùng một dashboard
  không thể mâu thuẫn nhau.
- `scale: 2` mặc định: ảnh chụp 1× của một chart nhìn rất mờ trong email trên màn hình hiện đại.

Kích thước lấy từ `custom_width`/`custom_height` của lịch, mặc định 1200×800 cho dashboard và
800×600 cho chart.

---

## 7. Định dạng, và một ràng buộc cần migration

`png` và `pdf` đi qua trình kết xuất. `csv` và `text` thì không — đó là dữ liệu backend đã có sẵn.

**Nhưng một dashboard không có bảng dữ liệu duy nhất.** CSV của "một dashboard" hoặc là chọn bừa một
card, hoặc là một file zip nhiều bảng, và cả hai đều tệ hơn việc nói thẳng ra. Do đó:

> `csv` và `text` chỉ hợp lệ cho báo cáo dạng **chart**. Báo cáo dạng **dashboard** bắt buộc phải là
> `png` hoặc `pdf`.

Đây là một CHECK constraint mới (P4 mang theo một migration) cộng với một luật kiểm tra gọi đúng tên
trường, theo đúng khuôn của P1: constraint là bảo đảm, validator là thông điệp.

---

## 8. Khi thất bại

| Tình huống | Kết quả |
|---|---|
| Không gọi được renderer / 5xx | `state=error`, ghi lý do, thử lại ở tick sau |
| Timeout chờ tín hiệu sẵn sàng | `state=error` — không bao giờ chụp dở |
| Chủ sở hữu mất quyền | `state=error` ngay khi cấp token, trước khi kết xuất |
| Token hết hạn hoặc bị phát lại | 404 khi đổi; kết xuất thất bại |
| Bản thân chart lỗi | Trang in tự đánh dấu thất bại; cờ sẵn sàng không bao giờ bật; timeout |

Một bộ từ vựng duy nhất, đã có sẵn trong ngăn kéo run log, và `next_run_at` được tính lại ở mọi
nhánh để một báo cáo đang lỗi vẫn tiếp tục thử chứ không chết hẳn.

---

## 9. Triển khai

- Image mới `egret-renderer:dev`, build bởi `just images-build` và nạp bởi `just deploy-kind`, là
  image thứ tư bên cạnh backend/frontend/kernel.
- Helm luôn cài Deployment + Service ClusterIP. Giới hạn bộ nhớ đặt vừa cho Chromium (đặt quá thấp
  sẽ thành OOMKill khi tải cao, và người dùng chỉ thấy "báo cáo lúc được lúc không").

---

## 10. Kiểm chứng thế nào

- **Unit/route**: token cấp đúng cho chủ sở hữu và không cho ai khác; đổi đúng một lần (lần thứ hai
  404); tôn trọng hạn dùng; endpoint payload từ chối đối tượng mà chủ sở hữu không đọc được; ràng
  buộc định dạng/đối tượng chặn dashboard+csv ngay ở API kèm thông điệp gọi tên trường.
- **Frontend**: route in chỉ bật cờ sẵn sàng sau khi chart xong VÀ font resolve — kiểm bằng cách lần
  lượt bỏ đi từng điều kiện, vì một cái cờ luôn bật mới chính là con bug.
- **Kiểm tra thật trên kind** — phần quyết định việc này có chạy hay không. Test không thể khẳng
  định "file PNG có chứa chart"; theo kinh nghiệm xuất PNG của chính repo này, hai đại lượng dùng
  được là **kích thước pixel và số byte** (kết quả 16×16 hay 32×32 nghĩa là đã chụp nhầm một icon
  giao diện). Vậy nên: đặt lịch một báo cáo trên dashboard thật, để nó chạy, rồi mở file đính kèm.

---

## 11. Thiết kế này KHÔNG làm gì

- **Không có bố cục in cho dashboard.** Mỗi lần kết xuất một đối tượng, kích thước cố định.
- **Không cache.** Hai lịch trên cùng một dashboard sẽ kết xuất hai lần. Cache ảnh của dữ liệu có
  phân quyền là một bài toán bảo mật riêng.
- **Không có filter tương tác lúc kết xuất.** Báo cáo vẽ theo trạng thái đã lưu của dashboard.
- **Không unfurl Slack/Teams.** Kênh gửi vẫn là email + webhook.

---

## 12. Rủi ro

- **Chromium trong pod nặng và hỏng theo những kiểu phần còn lại của egret không có** — thiếu font
  trong base image mỏng là ca kinh điển, và nó âm thầm biến chữ thành ô vuông chứ không báo lỗi.
  Chỉ kiểm tra thật mới bắt được; cờ sẵn sàng không bắt được.
- **Payload một-lần có thể rất lớn** với dashboard nhiều card. Bị chặn bởi đúng các giới hạn số dòng
  mà đường tương tác đang áp.
- **Một loạt lịch cùng chạy vào đầu giờ** mới là hình dạng tải thực tế, vì người ta hay viết
  `0 * * * *`. Giới hạn đồng thời cộng với giới hạn lô sẵn có của scheduler là thứ giữ cho nó không
  thành sự cố.
- **Token đi trong URL, mà URL thì bị ghi log.** Điều này không tránh được: renderer phải điều hướng
  tới một trang, và một lần điều hướng mang theo chứng thực ngay trên thanh địa chỉ. Thứ khiến nó
  chấp nhận được chính là phạm vi §5.3 đã chốt — dưới một phút, một lần dùng, một đối tượng, một
  chủ sở hữu. Đây là rủi ro được chấp nhận có ý thức, không phải sơ suất.
