# Chỉ báo focus trượt ngưỡng tương phản — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại
> `docs/en/plans/2026-08-05-focus-indicator-contrast.md` là bản chuẩn; mã nguồn, số dòng và các bước
> chạy lệnh nằm ở đó.

**Mục tiêu:** Làm cho chỉ báo focus bằng bàn phím đủ rõ để đạt ngưỡng của nó, ở cả hai theme, mà không
phá hai kiểu chỉ báo đang cùng tồn tại trong codebase.

**Kiến trúc:** Đây là thay đổi **design token**, không phải thay đổi component. Diff dự kiến rất nhỏ —
`--ring` trong `index.css`, và có thể là alpha của một hai utility. Toàn bộ phần còn lại của kế hoạch
này là **đo đạc và kiểm soát bán kính ảnh hưởng**, vì token đó với tới 41 tệp khắp sản phẩm, không chỉ
phần chart.

---

## Ngưỡng, nói rõ ngay từ đầu

**3:1, không phải 4.5:1.** Đây là chỉ báo focus **phi văn bản** — WCAG 1.4.11 và 2.4.11. Nói thẳng
điều đó trong PR chứ đừng lặng lẽ đo theo ngưỡng văn bản; một đơn vị trước trong chương trình này từng
sai theo chiều ngược lại và đã phải nói ra.

Phép so là với các màu **kề bên** — nền của chính component và nền trang phía sau — chứ không phải với
một màu trắng trừu tượng.

## Đã đo được gì, và ai đo

Một lượt soát accessibility tại `1d9b0d30` chuyển token OKLCH sang sRGB và ghép alpha bằng số, cố ý
tránh cái bẫy chương trình này từng dính: `getComputedStyle` trả về `oklch(...)` cho token của egret,
nên **một regex rgb ngây thơ sẽ đọc ba số đó thành RGB và cho ra tương phản rác**. #412 tự dẫn lại
bảng số một cách độc lập và khớp tới hai chữ số thập phân.

| | sáng | tối |
|---|---|---|
| `--ring` đặc | **2.58:1 TRƯỢT** | 4.08:1 đạt |
| `ring-ring/50` | **1.54:1 TRƯỢT** | **1.83:1 TRƯỢT** |
| fallback toàn cục `* { outline-ring/50 }` | **1.54:1 TRƯỢT** | **1.83:1 TRƯỢT** |

Token: sáng `--ring: oklch(0.71 0.01 286.09)` (`index.css:114`), tối
`--ring: oklch(0.55 0.02 285.93)` (`:165`), phơi ra thành `--color-ring` (`:208`). Fallback toàn cục là
`@layer base { * { @apply border-border outline-ring/50 } }` (`:245-247`).

**#412 đã chốt một câu hỏi để ngỏ bằng browser:** Chrome **có** tôn trọng `outline-color` do tác giả
đặt khi `outline-style: auto` — ép đỏ trên một phần tử đang focus rồi chụp lại vòng đỏ. Nên fallback
toàn cục không cần cách xử lý riêng.

## Chỗ phức tạp mà bảng số che mất

**Codebase đang trộn hai thế hệ shadcn**, và chúng sinh ra hai chỉ báo khác nhau:

- **kiểu cũ**: `focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2` — một vòng
  đặc 2px, cách control bởi một khe `ring-offset-background`. Ví dụ `ui/tabs.tsx:29`,
  `ui/switch.tsx:11`, `ui/radio-group.tsx:28`.
- **kiểu mới**: `focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px]` — chỉ
  báo **hai phần**: một viền `--ring` đặc cộng một quầng mờ 3px ở nửa alpha. Ví dụ `ui/badge.tsx:8`.

**Đo riêng cái quầng `/50` rồi kết luận là chưa đủ** — ở kiểu mới, chính viền `border-ring` đặc mới là
thứ mang tương phản, còn quầng là trang trí. Hôm nay cả hai vẫn trượt (viền 2.58:1), nhưng **cách sửa
thì khác nhau**: nâng `--ring` sửa thẳng phần viền của kiểu mới, còn kiểu cũ thì vòng của nó vốn cũng
đã dùng `--ring` đặc. Quầng `/50` có thể **không cần** sửa gì cả.

**Xác lập điều này trước khi đổi bất cứ thứ gì.** Nếu kết luận là "một thay đổi token sửa được mọi
kiểu" thì đó là kết cục tốt nhất và diff chỉ là ba dòng.

> **ĐÍNH CHÍNH, từ Task 1 (đây là điều việc đo thực sự tìm ra).** Đoạn trên đúng với `Input`,
> `Badge`, `Textarea` và `SelectTrigger`, nhưng **sai với `Button`** — đúng cái control mà quy tắc này
> quan trọng nhất. `focus-visible:border-ring` chỉ đặt *màu* viền; nó không vẽ được gì nếu phần tử
> chưa có *độ dày* viền. Các variant `default`, `ghost`, `link` của Button không có: đo trong Chrome,
> `border-top-width` của một nút header đang focus là `0px`, nên với các variant đó **không hề có
> phần đặc nào** và **quầng `/50` chính là toàn bộ chỉ báo**, ở mức 1.54:1 sáng / 1.83:1 tối.
>
> Làm token tối đi không cứu được: một phép trộn 50% chỉ đạt tối đa khoảng 1.73:1 dù `--ring` tối đến
> mấy, vì nó bị pha nửa-nửa vào chính mặt nền. Vậy câu trả lời cho "một hay hai bản sửa" là **hai vấn
> đề, ba dòng**: làm tối token *và* bỏ alpha ở đúng hai chỗ mà một vòng giảm-alpha là chỉ báo duy
> nhất — fallback toàn cục `* { outline-ring/… }` và `Button`. Bốn primitive còn lại giữ nguyên quầng
> mờ, vì viền đặc của chúng thực sự gánh được một khi token đã đạt.

## Bán kính ảnh hưởng, đã đếm

41 tệp: `ring-ring` ở **33**, `border-ring` ở **5**, `outline-ring` ở **3** (tổng cộng 74 lần xuất hiện
`focus-visible:ring`). Theo khu vực:

| khu vực | số tệp |
|---|---|
| `components/ui` | 12 |
| `components/chart` (+ `settings`, `__tests__`) | 11 |
| `components/sqllab` | 4 |
| `routes/explore` (+ `settings`) | 2 |
| `routes/control-plane` (config, access) | 2 |
| `components/notebook` | 1 |

**Chỉ khoảng một phần tư là chart.** PR này ra ngoài lãnh địa của chương trình chart, và người review
cần biết điều đó.

---

## Task 1 — tự đo, trong browser, trước khi đổi gì

**Đừng tin bảng số trên.** Nó là thông tin qua tay hai lần, và đây là thay đổi token với tới mọi phần
tử focus được trong sản phẩm.

Tự dẫn lại các con số (vẽ màu lên canvas 1×1 rồi đọc ngược, hoặc chuyển OKLCH→sRGB bằng số — **nói rõ
đã dùng cách nào**; **đừng regex `oklch()`**). Rồi **đo chỉ báo như nó ĐƯỢC VẼ RA, theo từng kiểu, chứ
không theo từng token**: focus một control thật trong browser thật và đo đúng những pixel của chỉ báo
so với đúng những pixel kề bên. Kiểu cũ có khe `ring-offset-background` giữa control và vòng — nói rõ
vòng đó thực sự **kề với cái gì**.

Và **kiểm `--ring` có được dùng cho thứ gì không phải chỉ báo focus không.** Nếu có, làm nó tối đi sẽ
đổi một thứ không phải focus, và điều đó phải được báo lại.

**Báo cáo trước khi làm tiếp.**

## Task 2 — đổi token

Viết test đỏ trước: test có thể tự tính tương phản của token mà không cần browser, vì `index.css` đọc
được và phép toán là tất định. Ghim **cả hai theme** và ghim ngưỡng **3:1** kèm comment nói vì sao là 3
chứ không phải 4.5.

Làm tối `--ring` của theme sáng. Ước lượng chưa kiểm của lượt soát là khoảng `oklch(0.63 …)` — **tự dẫn
ra giá trị thật** chứ đừng lấy nguyên. **Giữ nguyên hue và chroma** — đây là token trung tính, đổi màu
là một thay đổi thị giác không ai yêu cầu.

Theme tối hôm nay đạt 4.08:1. Nếu có đụng vào giá trị tối thì nó vẫn phải đạt; nếu để nguyên thì nói rõ.

Quyết định về alpha `/50` dựa trên kết quả Task 1: nếu viền đặc mang chỉ báo thì để quầng ở nửa alpha
là đúng, và **phải nói ra**; nếu không thì nâng lên và nói cái giá thị giác.

## Task 3 — nhìn nó

Một con số tương phản không phải là một lượt review thiết kế. Chương trình này có sẵn bài học đúng chỗ
đó: **đo số không thay được nhìn ảnh** — caption của scalar đạt mọi phép kiểm bằng số trong khi nằm
cách con số nó chú thích 300px, và chỉ ảnh chụp mới thấy.

Chụp một control đang focus ở **cả hai theme**, trước và sau, cho **cả hai thế hệ**. Kiểm xem nó có
thành ra đọc như một cái viền thay vì một vòng focus không — vòng tối hơn trên nền sáng thì rõ hơn theo
cấu tạo, rủi ro nằm ở chỗ nó trông nặng, hoặc va với `--border` của phần tử kề bên.

Và **kiểm một control trên nền KHÔNG trắng** — một mục đang focus bên trong popover, một dialog, một
card tối. Lượt soát đo với nền trang; chỉ báo phải chạy được trên những bề mặt nó **thật sự** rơi vào.

## Task 4 — quét và mở PR

Tab qua một màn hình ở **mỗi khu vực bị ảnh hưởng** — một panel settings của chart, SQL Lab, một
notebook, một trang control-plane. Không cần mọi tệp; một đại diện mỗi khu vực, và nói rõ chọn cái nào.
Mục đích là bắt được khu vực nào mà token đó mang ý nghĩa khác.

PR phải nói thẳng đây là thay đổi **cấp sản phẩm**, chỉ một phần tư số tệp nằm dưới `components/chart/`,
và liệt kê các khu vực bị chạm, để người review biết đây không phải PR về chart.

---

---

## Kết quả — đã đo gì, đổi gì, và để yên gì

**Đo bằng ba đường độc lập** trước khi đổi bất cứ thứ gì: chuyển OKLCH→sRGB bằng số (Ottosson); tô
đúng các màu đó lên canvas 1×1 trong Chrome thật rồi đọc ngược ra; và — phần chưa ai làm — **quét
chính các pixel đã vẽ của vòng focus** từ ảnh chụp các control được Tab tới bằng phím thật. Cả ba
khớp nhau tới mức làm tròn 8-bit, và tái lập đúng bảng số của audit. (Lượt canvas cũng xác nhận
`--card` chế độ tối có độ sáng tương đối **0.0094** — đúng giá trị mà chương trình này đã ghi nhận là
giá trị thật.)

**Chỉ báo khi đã vẽ, theo từng thế hệ, chế độ sáng, tính từ mép control ra ngoài:**

| thế hệ | các dải pixel | kết luận |
|---|---|---|
| cũ (`ring-2` + `ring-offset-2`) | viền card `#e4e4e4` → **khe trắng 2px** → **vòng `#a1a1a8` 2px** → nền trang trắng | vòng kề màu trắng ở **cả hai phía**, **2.57:1 TRƯỢT** |
| mới, có viền (`Input`) | nền trắng → **viền `#a1a1a8` 1px** → quầng `#cfcfd3` 3px **inset** → ruột field trắng | viền gánh chỉ báo, **2.57:1 TRƯỢT**; quầng không hề chạm nền trang |
| mới, không viền (`Button`) | nền trắng → quầng `#cfcfd3` 3px → ruột nút trắng (không có viền nào) | **quầng là toàn bộ chỉ báo, 1.54:1 TRƯỢT** |
| không có style focus riêng | `outline-style: auto` của Chrome, màu `--ring/50` | **1.54:1 TRƯỢT**, và đây là trường hợp chiếm đa số |

Dòng cuối là dòng mà con số bán kính ảnh hưởng đã đánh giá thấp. Tab qua các trang control-plane,
**11 trong 14 điểm dừng đầu tiên** — toàn bộ thanh điều hướng bên trái — không có style focus riêng và
rơi vào fallback toàn cục.

**Thay đổi (ba dòng có tác dụng):**

1. `--ring` sáng: `oklch(0.71 …)` → `oklch(0.63 …)`. 0.63 không phải lấy từ ước lượng của audit, mà
   là độ sáng đầu tiên vượt 3:1 trên **mọi** mặt nền mà một control đang focus có thể nằm lên:
   background/card 3.51, popover 3.31, accent 3.21, dashboard 3.07, **secondary 3.03**. Nếu chỉ đo
   với `--background` thì ~0.67 đã được chấp nhận, trong khi nó trượt trên `--secondary` ở 2.91.
2. fallback toàn cục: `outline-ring/50` → `outline-ring`.
3. `Button`: `focus-visible:ring-ring/50` → `focus-visible:ring-ring`.

**`--ring` chế độ tối không bị đụng vào** — nó vốn đã đạt (4.08 trên background, 3.09 ở mặt nền xấu
nhất), và pixel chế độ tối trước/sau giống nhau từng byte.

**Sau khi sửa, ở mức pixel:** vòng của kiểu cũ đi từ `#a1a1a8` → `#88888f` (2.57 → **3.52:1**); pixel
lõi của fallback toàn cục nay là `#88888f` ở chế độ sáng (**3.52:1**, trước là 1.54) và `#70707d` ở
chế độ tối (**3.62:1**, trước là 1.83). Trên 88 điểm dừng Tab thuộc bốn khu vực, mọi điểm dừng đều vẽ
ra token ring và không còn điểm nào dùng chỉ báo nửa-alpha.

**Nó có bắt đầu trông như một cái viền không?** Không, và các con số đi đúng hướng: `--ring` so với
`--border` cải thiện từ 2.04:1 lên **2.77:1**, nên vòng focus giờ *dễ* phân biệt với viền tĩnh hơn
trước, chứ không khó hơn. Kiểu cũ cũng vẫn giữ khe offset, nên vẫn đọc ra là "vòng" chứ không phải
một cái mép dày lên.

## Phát hiện khi đo, cố ý KHÔNG sửa ở đây

Mỗi mục dưới đây đều là lỗi thật; không mục nào là vấn đề của token tương phản, và gộp bất kỳ mục nào
vào cũng sẽ làm PR này không review nổi.

- **Item trong menu đang focus gần như không hiện gì.** Một item dropdown trên `--popover` không dùng
  `--ring` chút nào — nó báo focus bằng nền `--accent`, đo được **1.03:1** so với popover (`#f5f5f5`
  trên `#f8f8f8`). Người dùng bàn phím gần như không thấy mình đang ở đâu trong menu. Đây là vấn đề
  của `--accent`/`--popover` và cần một quyết định riêng.
- **`ring-offset-color` mặc định là `#fff`** ở mọi call site dùng `ring-offset-2` mà không kèm
  `ring-offset-background` (ví dụ `AccessOverview.tsx:376`). Ở chế độ tối, nó vẽ ra một **dải trắng
  tinh 2px** bên trong vòng. Vô tình làm vòng *dễ thấy hơn*; nhưng rõ ràng là ngoài ý đồ.
- **`resizable.tsx:40` có `focus-visible:ring-ring` mà không có độ dày ring**, nên nó đặt màu và
  không vẽ ra vòng nào cả.
- **`SqlLab.tsx:35` là chỗ duy nhất dùng `--ring` không phải cho focus** (`ring-1 ring-ring`, không có
  `focus-visible:`). Nó vượt qua kiểm tra Task 1 Bước 3: input chỉ được mount trong lúc đổi tên và
  unmount khi blur, nên vòng chỉ hiện đúng lúc control đang focus — là chỉ báo focus theo mọi nghĩa
  trừ cái tên. "Điểm yếu đã biết" của kế hoạch đã không xảy ra.
- **Hai thế hệ shadcn vẫn để nguyên chưa gộp**, đúng như kế hoạch chỉ định.

## Một cái bẫy đáng ghi lại

Tailwind v4 quét file nguồn để tìm ứng viên tên class và **không phân biệt được comment với code**.
Viết tên utility cũ trong comment WHY ngay phía trên bản sửa đã khiến Tailwind phát ra một rule
`outline-color: color-mix(… 50% …)` thật vào bundle — rule chết, vì không có gì dùng class đó, nhưng
khi grep bundle thì nó trông y hệt như bản sửa đã không vào. Cùng điểm mù đó cắn luôn cả test: bản
nháp đầu tiên đọc alpha từ chính comment của nó và ba mutation sống sót. Giờ comment bị bóc trước khi
parse, và giá trị cũ không còn được viết ra ở bất cứ đâu trong `index.css`.

## Cố ý nằm ngoài phạm vi

Số của trend tràn card hẹp; tiêu đề trục trái của combo đọc tên series đã ẩn sau khi click legend;
việc thiếu đường bàn phím tới drill theo ô, thứ mà `ChartTable`, `PivotTable` và object detail đều
thiếu như nhau và là **một quyết định** chứ không phải ba bản vá; và các khoảng cách tính năng phía
Metabase.

## Tự soát

- **Diff phải nhỏ, còn phần đo thì không.** Nếu việc này phình thành một diff lớn thì có gì đó đã chẩn
  sai — dừng lại và báo, đừng cố đi tiếp.
- **Task 1 là điều tra và kết thúc bằng một báo cáo**, vì các con số nền tảng là thông tin qua tay và
  thay đổi thì cấp sản phẩm. Lượt soát sinh ra chúng đã cẩn thận và #412 đã đối chứng — nhưng đối chứng
  bởi một người dẫn lại **từ cùng bộ token** không giống với việc đo **chỉ báo đã vẽ ra**, thứ chưa ai
  làm.
- **Chỗ yếu đã biết:** kế hoạch giả định làm tối `--ring` của theme sáng là đủ và an toàn. Nếu `--ring`
  hoá ra còn kiêm một cái viền không-phải-focus ở đâu đó thì giả định đó gãy và bản sửa thành một token
  mới — Task 1 sinh ra để phát hiện sớm điều đó.
- **Đừng "cải tiến" style focus khi đang ở trong đây.** Hai thế hệ shadcn đáng được hợp nhất, nhưng đó
  là một phần việc riêng với lượt review riêng; trộn nó vào một bản sửa tương phản sẽ làm cả hai khó
  đánh giá.
