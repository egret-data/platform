# Redesign Chart 01 — Bar

**Trạng thái:** thiết kế đã duyệt, chờ kế hoạch triển khai
**Date:** 2026-07-28
**Spec chương trình:** `docs/en/specs/2026-07-28-chart-redesign-program-design.md`
**Bản tiếng Anh (canonical):** `docs/en/specs/2026-07-28-chart-redesign-01-bar.md` — bảng giữ nguyên; bản này dịch phần diễn giải.

## Bar dùng để làm gì

So sánh một đại lượng trên một tập nhỏ các hạng mục. Mọi thứ bên dưới suy ra từ đó:
hạng mục phải đọc được, giá trị phải so sánh được (trục bar bắt đầu từ 0), và một
cái đuôi dài không được biến thành hai trăm vạch không đọc nổi.

Bar làm trước vì nó được dùng nhiều nhất, và vì bốn thay đổi kiến trúc của chương
trình — control có nhóm, cartesian khai theo từng loại, mặc định ở tầng render, và
hai helper dùng chung — đều đáp xuống ở đây để các chart sau kế thừa.

## Phải đúng ngay khi chưa cấu hình

| | Hôm nay | Sau khi sửa |
|---|---|---|
| Legend | shown only if `showLegend` is set; default off | on when there is a breakout or more than one metric, off otherwise |
| Category labels | drawn at full length, overlapping when crowded | thinned, and rotated when they still collide |
| Value axis | full numbers (`25,468`) | compact (`25.5K`) when the axis is narrow |
| Long tail | every category drawn | unchanged by default — see below |
| Axis titles | empty unless typed | derived from the column and metric names |

**Gộp đuôi dài là tuỳ chọn, không mặc định.** Metabase để
`graph.max_categories_enabled` mặc định `false` và `max_categories` bằng
`MAX_SAFE_INTEGER` — tức là mời chứ không ép. egret theo đúng vậy: control có mặt,
mặc định giữ nguyên mọi hạng mục. Âm thầm giấu dữ liệu sau chữ "Other" còn tệ hơn
một trục chật.

## Kiểm kê control

`group` is the new `VizControl.group`; `advanced` is the collapsed section.

| Control | Nhóm | Mặc định | Thay đổi |
|---|---|---|---|
| X-axis | Data | — | unchanged |
| Y-axis (metrics) | Data | — | unchanged |
| Series (breakout) | Data | none | unchanged |
| Max categories + Other | Data | off | **new** |
| Series order / visibility | Series | source order | **new** |
| Series colours | Series | palette | moved from a flat list |
| Series labels | Series | metric name | moved |
| Stacked | Series | off | moved |
| 100% stacked | Series | off | moved, still gated on Stacked |
| Stack totals | Series | off | moved, still gated |
| Horizontal | Axes | off | moved |
| X axis scale | Axes | ordinal | moved |
| X / Y axis title | Axes | derived from column names | **default changed** |
| Y min / Y max | Axes, advanced | auto | moved |
| Log scale | Axes, advanced | off | moved |
| Data labels | Format | off | moved |
| Label density | Format | auto | **new** |
| Compact numbers | Format | auto | **new** |
| Goal line + label | Format, advanced | none | moved |
| Trend line | Format, advanced | off | moved |

Không bỏ control nào. Sáu control chuyển vào nhóm; ba cái mới; một mặc định đổi.

## Khoảng cách chart này lấp

From the program comparison, the two that apply to bar:
`graph.max_categories` (+ the "Other" bucket) and `graph.series_order`. Both are
built as shared code — the fold helper and the series-order control — so line,
area and combo inherit them.

`graph.tooltip_columns` and `y_axis.auto_split` are **not** part of this chart:
tooltips belong to a pass of their own, and auto-split matters for combo, where
it will be designed.

## Cố ý không làm

- **Legend click-to-hide** — ECharts' legend already toggles a series on click.
  The program's standard 4 is satisfied here without code; it will be verified
  live rather than reimplemented.
- **Per-bar colour by value** (a conditional-formatting-style ramp). Metabase
  does not do it either, and it invites charts that mislead.
- **Bubble size** — scatter's, not bar's.

## Live check

On the Demo Sales dataset:

1. `region` × `total_revenue` with no other configuration: four bars, no legend,
   compact axis labels, both axis titles filled in.
2. Add `category` as a breakout: the legend appears by itself; clicking an entry
   hides that series.
3. Switch to a high-cardinality dimension (`order_date`, day grain): labels thin
   out rather than overlapping; turning on Max categories folds the tail into
   "Other".
4. Reorder two series, then reload the saved chart: the order survives.
5. The same chart as a small dashboard card: still legible, with fewer labels.
6. A bar chart saved before this change still opens and now picks up the new
   defaults (they are render-time, so this is the case that proves it).

## Kiểm chứng

The program's gate: lint, typecheck, tests, build, deploy to kind, live check,
plus before/after screenshots at both sizes.
