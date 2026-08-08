# "Hide this card if there are no results"

Cái cuối trong ba setting của dashcard modal Metabase mà egret chưa có. Modal của Metabase có
**Title**, **Description** và **Hide this card if there are no results** trên mọi chart; panel card
settings của egret có Title, On click, Auto height, Move to tab.

Đơn vị này **đã dựng** hide-if-empty và **không** dựng Description. Cả hai quyết định đều được lập
luận bên dưới. (Bản tiếng Anh là bản chuẩn.)

## Cờ của Metabase thực sự làm gì — đo, không phải đọc

Chạy trên một dashboard nháp dựng riêng cho việc này (không bao giờ là `/dashboard/2`, vốn là mốc
parity), một hàng 24 cột gồm ba card `8×6`, card giữa có SQL trả về không dòng nào:

| | |
|---|---|
| cờ TẮT | tile vẽ trạng thái rỗng **"No results!"**, đủ cả hình thuyền buồm |
| cờ BẬT | **không có tile nào cả** — không phải trạng thái rỗng, không phải chỗ giữ chỗ |
| hàng | `Bar │ EMPTY │ Row` ở cột 0/8/16 vẽ thành `Bar │ … │ Row` — **lỗ hổng vẫn còn** |
| card bên dưới | card ở row 6, khi card row 0 bị ẩn, **vẽ ở row 0** |

Vậy Metabase dồn **theo chiều dọc chứ không theo chiều ngang**. Đó không phải mã đặc biệt phía họ:
`DashboardGrid` truyền `compactType="vertical"` cho react-grid-layout, và đó đơn giản là điều RGL làm
khi một child biến mất. Bản thân setting là `card.hide_empty`, lưu trên DASHCARD, và
`getVisibleCardIds` (`dashboard/utils.ts:359`) lọc id ra khỏi tập được render. Nó từ chối ẩn card đã
LỖI (`!getDashcardResultsError(...)`), và trong lúc dữ liệu đang tải nó trả `!wasVisible`, tức card
chưa ai thấy sẽ bắt đầu ở trạng thái ẩn thay vì vẽ ra rồi biến mất.

## Ước lượng, và chỗ đề bài sai

Đề bài dự đoán lưới là phần đắt — *"card bị ẩn để lại lỗ trừ khi layout dồn lại"* — và bảo hãy ước
lượng-rồi-dừng nếu nó dính tới layout.

**Hoá ra lưới là miễn phí.** `DashboardGridLayout.tsx:64` của egret vốn đã truyền đúng
`compactType="vertical"`, và `visibleCards` vốn đã nuôi CẢ mảng `layout` của RGL LẪN các children của
lưới. Bỏ một id khỏi đúng một danh sách đó tái tạo cả hai nửa hành vi đã đo của Metabase mà không cần
một phép tính layout nào. `onLayoutChange` của egret cũng vốn đã return sớm khi `!editing`, nên việc
dồn ở chế độ xem chỉ là hiển thị và không thể ghi đè `grid_row` đã lưu.

Chi phí thật nằm ở chỗ đề bài không nhìn tới: **các dòng dữ liệu được nạp BÊN TRONG tile.**
`ChartCardTileView` gọi `useDashboardCardData`, nhưng `visibleCards` được tính ở `DashboardView`, nơi
không hề truy cập được kết quả của card nào. Cách làm hiển nhiên — để tile tự nhận ra mình rỗng rồi
báo cha bỏ nó đi — **tháo bỏ chính cái hook đã tạo ra câu trả lời**. Card biến mất, ngừng truy vấn, và
không bao giờ biết được rằng mình đã có dữ liệu trở lại; nó sẽ ẩn cho tới khi tải lại cả trang. Đó là
lỗi đúng-sai, không phải chuyện trau chuốt, và đó là toàn bộ lý do đơn vị này có `CardEmptinessProbe`.

## Đã dựng gì

- `hide-empty.ts` — `hiddenEmptyCardIds(cards, emptiness, editing, blocked)`, toàn bộ quyết định,
  thuần và do đó kiểm thử được. Chỉ card chart; không ẩn gì khi đang sửa; card chưa có kết quả tính là
  rỗng, khớp `!wasVisible` của Metabase.

  Hai chốt chặn **không** có trong bản đầu và được phát hiện nhờ đọc lại diff chứ không phải do test
  nào đỏ — cả hai đều là trường hợp "chưa biết" không có nghĩa "đang tải" mà là "sẽ không bao giờ
  được trả lời", và mặc định chưa-biết-thì-ẩn sẽ ẩn card vĩnh viễn:

  - card chart có `chart_id` null không chạy truy vấn nào, nên không bao giờ có báo cáo. Cái đúng
    phải hiện là "Chart not found" của chính tile đó — cùng lý lẽ giữ card lỗi ở lại trên bảng.
  - dashboard có **tham số bắt buộc chưa đặt giá trị** chặn mọi card, chúng hiện "Set the required
    filters to load this card" và không truy vấn. Không có chốt `blocked`, cả bảng sẽ biến mất sau
    thanh lọc mà không một lời giải thích.

  `probeCards` trong `DashboardView` mang đúng cùng vị từ đó, và bắt buộc phải vậy: nếu hai bên lệch
  nhau, một card có thể bị ẩn mà không còn gì chạy để bỏ ẩn nó.
- `CardEmptinessProbe.tsx` — chạy truy vấn của card NGOÀI lưới và báo ngược lên. Render `null`. Nó
  **không tốn thêm request**: `useDashboardCardData` là `useQuery` của react-query đánh khoá theo
  `["dashboardsDbCardRun", ws, dashboardId, cardId, nonce, filters, temporalUnits]`, nên probe và
  tile của một card đang hiện dùng chung một mục cache. **Đã kiểm chứng trực tiếp** — bảng hai card
  phát đúng một request `/cards/{id}/data` cho mỗi card. `DashboardView.cardQueryArgs()` là nguồn duy
  nhất của các tham số đó chính để hai bên không lệch nhau rồi nạp đôi.
- `CardSettingsPanel` — công tắc, dùng **đúng nguyên văn chữ của Metabase**.
- Một trạng thái rỗng "mọi card đều tự ẩn", vì rơi vào câu *"No cards yet. Click Edit to add charts."*
  sẵn có sẽ là nói dối trên một bảng vốn có card.

Chỉ frontend. `options` là blob `jsonb` tự do từ đầu tới cuối (`migrations/0015_dashboard.sql:24`),
nên việc này **không cần đổi Rust, không cần migration, không cần sinh lại OpenAPI**.

## Đã kiểm chứng trực tiếp

Build `dist`, phục vụ qua reverse proxy tới `:30080` của kind (đã đối chiếu hash bundle với bản
build), dashboard nháp trong `demo`, hai card, chỉ card bên phải bật hide-if-empty:

1. không lọc — cả hai card đều vẽ, nên bật cờ không ẩn card đang có dữ liệu;
2. đặt bộ lọc thành giá trị không khớp gì — **card bên phải biến mất**, còn card trái giữ nguyên cột
   và để lỗ bên phải, đúng như Metabase;
3. xoá bộ lọc — **card quay lại**, không cần tải lại trang. Đây là bước cách làm ngây thơ sẽ trượt, và
   là lý do probe tồn tại.

Mốc parity được kiểm lại sau đó và không bị đụng: 22 card, `updated_at 2026-08-08T01:03:49.111686Z`,
`vizOverride` 0, `hideEmpty` 0. Dashboard nháp đã bị xoá.

## Description — quyết định riêng, và KHÔNG dựng

Nó nhỏ hơn hide-if-empty về logic và lớn hơn về bề mặt, nên nó không thuộc đơn vị này.

Bản thân setting là thêm một textarea cạnh Title ghi `options.description` — năm dòng, không hề có
vấn đề luồng dữ liệu, vì tile đã có sẵn mọi thứ nó cần. **Nhưng không có chỗ nào để vẽ nó ra.** Header
tile của egret chỉ có tiêu đề (`DashboardCardView.tsx:55-71`): không icon info, không tooltip. Metabase
hiện mô tả của dashcard sau một icon info ở header card. Nên ship Description nghĩa là phải dựng cái
affordance header đó — UI hoàn toàn mới, cần một lượt đối chiếu parity riêng — cộng thêm một quyết
định mà title đã có lời giải còn description thì chưa: chart CÓ trường `description`
(`api/charts.ts:494`) mà chưa tile dashboard nào từng vẽ, nên mô tả cấp card phải định nghĩa rõ là nó
ghi đè hay rơi về mô tả của chart, theo cách `title` rơi về `chart.name`.

**Khuyến nghị: ước lượng Description cùng phần header tile / metadata card, không phải cùng phần
này.** Nó là một setting và một icon, nhưng cái icon mới là phần chưa từng tồn tại.

## Tìm thấy, chưa sửa

- Bộ chọn giá trị tham số dashboard của egret chỉ đưa ra các giá trị có trong dữ liệu, nên không có
  cách nào đẩy một card về "không kết quả" chỉ bằng UI. Phần kiểm chứng ở trên phải đặt tham số qua
  query string của URL. Không phải lỗi — nhưng nghĩa là người dùng khó tự *thấy* setting
  hide-if-empty của mình có tác dụng, đáng nhớ nếu có ai báo là nó hỏng.
