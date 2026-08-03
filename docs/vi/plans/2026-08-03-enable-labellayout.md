# Đưa `LabelLayout` vào bundle — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại `docs/en/plans/2026-08-03-enable-labellayout.md`
> là bản chuẩn; mã nguồn và các bước chạy lệnh nằm ở đó.

**Mục tiêu:** Làm cho `labelLayout: { hideOverlap: true }` thật sự có tác dụng. Sáu tệp đặt nó, và
không tệp nào có nó trong app đã ship — kể cả một setting mà panel chào ra cho người dùng bằng tên.

**Kiến trúc:** Một dòng trong `echarts/register.ts`. Công việc không nằm ở thay đổi đó, mà ở việc xác
định nó làm gì với năm họ chart **trước khi** ship.

---

## Lỗi

ECharts cài label layout bằng `use(installLabelLayout)` ở cấp cao nhất trong `lib/export/core.js`.
Đường dẫn đó **không** nằm trong danh sách `sideEffects` của `package.json` echarts, nên Rollup loại bỏ
lời gọi.

Xác nhận trong `frontend/dist/assets/index-*.js`: `shiftLayoutOnXY`, `labelGuideIgnore`, `hostRect` và
`installLabelLayout` đều **0 hit** — `LabelManager.js` không hề có trong bundle.

**vitest và `vite dev` không tree-shake**, nên mọi unit test và mọi phiên dev đều cho thấy chốt chặn
đang hoạt động. Hai test SSR và một trang Chromium độc lập đã xác nhận `hideOverlap` chạy tốt với
`sunburst` — cơ chế không sai, bản build mới là thứ loại nó đi.

**Chính `register.ts` đã có tiền lệ, bằng lời của nó:**

> Component aria không được re-export từ "echarts/components", và entry sâu này tự đăng ký khi import.
> Thiếu nó thì mọi option `aria` bị **âm thầm bỏ qua**, và đó đúng là chuyện đã xảy ra ở đây cho tới
> khi cuộc kiểm định accessibility đi tìm cái nhãn và không thấy gì.

Cùng lớp lỗi, cùng tệp, phát hiện theo cùng cách.

## Nó chạm tới đâu — rủi ro không đối xứng

| tệp | `hideOverlap` chi phối gì | mặc định bật? |
|---|---|---|
| `options/cartesian.ts:991` | **nhãn giá trị** của series, khi `labelDensity !== "all"` | **không** |
| `options/waterfall.ts:320` | nhãn giá trị, `labelDensity ?? "fit"` | **không** |
| `options/boxplot.ts:427` | nhãn giá trị của điểm trung bình | **không** |
| `options/pie.ts:1710` | nhãn lát | **có** — `cfg.showLabels ?? true` |
| `options/sankey.ts:354` | nhãn nút | **có**, luôn vẽ |

Vậy:

- **Nửa nhãn giá trị là một setting nói dối.** `CartesianDisplaySection.tsx:427`,
  `WaterfallDisplaySection.tsx:76` và `BoxplotDisplaySection.tsx:75` chào ra select mật độ `fit` / `all`
  ngay khi người dùng bật nhãn giá trị. Hiện `fit` và `all` render **giống hệt nhau**. Đây là nửa người
  dùng thấy được, và bật component lên thì vẫn vô hình cho tới khi có ai đó chủ động bật — rủi ro thấp.
- **Nửa pie và sankey đổi thứ đang hiển thị**, không cần người dùng làm gì. Rủi ro nằm ở đó, và Task 1
  sinh ra để đo nó.

## Thứ KHÔNG được đổi

PR 05 và 05b đã làm quyết định nhãn của pie và sankey thành **hình học và chính yếu**, chính vì
`hideOverlap` đã chết:

- `sliceLabel.ts` đo từng nhãn với dây cung của chính lát đó rồi cắt hoặc bỏ.
- `sankey.ts` dành khoảng chừa bên phải cho nhãn dài nhất của cột cuối và cắt ở ngưỡng 25%.

Hai thứ đó vẫn là chính yếu. `hideOverlap` là chốt chặn cho những nhãn vượt qua chúng mà vẫn đè nhau.
`options/pie.ts:1706-1709` đã nói vậy: *"Every decision about what fits is made above, where it can be
measured; this line is what the app gets back for free if that install is ever restored."*

**Nếu bật nó lên mà một nhãn pie hay sankey mà phép fit hình học cố ý GIỮ lại bị bỏ đi, đó là hồi quy
của một quyết định đã được đo** — phải báo cáo chứ không mặc nhiên chấp nhận.

## Các task

| # | nội dung |
|---|---|
| 1 | **Đo trước, chưa đổi gì** — build hai cách, diff kết quả render trên năm họ chart × nhiều kích thước, báo cáo rồi dừng |
| 2 | Bật nó lên |
| 3 | Đối chiếu live + mở PR (KHÔNG merge) |

### Task 1 phải trả lời

- Lỗi còn không (kiểm bốn ký hiệu trong bundle đã ship).
- Export đúng là gì (`echarts/features` xuất `LabelLayout`, echarts 6.1.0).
- **Nhãn nào biến mất**, theo từng họ chart và từng kích thước — tối thiểu card 166×184 của harness và
  một khổ rộng (640×400), vì một chốt chặn không kích hoạt ở kích thước này có thể kích hoạt ở kích
  thước khác. Phủ các họ nhãn giá trị với `showLabels: true` và **cả** `labelDensity: "fit"` lẫn
  `"all"` — điểm mấu chốt là hai giá trị đó hiện render giống nhau và lẽ ra không được thế.
- **Bundle to thêm bao nhiêu.**

### Vấn đề kiểm thử, nêu thẳng chứ không lấp liếm

**Một unit test không thể fail vì lỗi này**, bởi lỗi chỉ tồn tại trong bản build đã tree-shake. Test
render qua ECharts thật trong SSR sẽ xanh **ngay hôm nay**, vì vitest không tree-shake — nên nó **không**
phải test hồi quy cho lỗi này. Cách duy nhất có thể fail đúng lý do là **kiểm chính bundle sau khi
build**. Task 2 phải quyết định chỗ đặt phép kiểm đó và nói rõ nó chứng minh được gì và không chứng
minh được gì.

Chương trình này đã hai lần ship một suite xanh phủ lên một nhánh mã đã chết.

## Tiêu chí nghiệm thu (Task 3)

1. **Bundle đã ship chứa `LabelManager`** — kiểm lại bốn ký hiệu.
2. **`fit` và `all` phải KHÁC nhau** trên `MB · Bar` với "Data labels" bật, ở khổ hẹp. Hôm nay chúng
   giống nhau. Báo cáo cả hai tập nhãn.
3. **Không chart nào trên dashboard hồi quy** — đối chiếu bảng số của PR 02–07: sankey `Doo… Wid… Gad…
   Gizmo` và chín màu, pie không vẽ nhãn lát ở 166×129, row `0 / 500K`, gauge một vạch, pivot 14 hàng,
   trục thời gian đọc `January 2026`, và thứ tự danh mục của bar/row/box plot.
4. **Một nhãn biến mất là một phát hiện, không mặc nhiên là thắng lợi.** Nếu pie hay sankey mất một nhãn
   mà phép fit hình học đã giữ, báo cáo kèm kích thước xảy ra.

**Kiểm hash bundle** trước khi tin bất kỳ phép đo nào.
