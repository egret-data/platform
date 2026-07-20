# Ví dụ: Notebook dashboard thương mại điện tử

Một notebook khép kín, sao-chép-là-chạy, tái hiện một "dashboard" thương mại điện
tử kiểu Metabase — các chỉ số doanh thu (KPI), sản phẩm bán chạy, và doanh thu
tách theo danh mục và theo tháng — chỉ dùng kernel (Daft). Nó **không cần engine,
warehouse hay dữ liệu seed nào**, nên chạy được trong mọi workspace và đồng thời
là một smoke test cho notebook (xem `frontend/tests/e2e/notebook-ecommerce.spec.ts`).

Nó bao trọn luồng phân tích ad-hoc: chạy Python, `daft.from_pydict`,
`daft.sql(...)` trên các DataFrame đã bind (JOIN / SUM / GROUP BY / ORDER BY),
render bảng bằng `display()`, và cell magic `%%egret_sql --target daft`.

## Bộ dữ liệu

Một cửa hàng nhỏ: ba sản phẩm thuộc hai danh mục, mười hai đơn hàng trong ba
tháng. Các con số dưới đây là tất định:

| Chỉ số | Giá trị |
| --- | --- |
| Tổng doanh thu (Σ giá·SL) | **255.0** |
| Giá trị đơn trung bình (255 / 12) | **21.25** |
| Số khách hàng khác nhau | **6** |
| Sản phẩm bán chạy nhất | **Gadget** (125) |
| Doanh thu theo danh mục | Gizmos **195**, Gadgets **60** |
| Doanh thu theo tháng | 2026-01 **75**, 2026-02 **110**, 2026-03 **70** |

## Cell 1 — dựng dữ liệu + KPI dashboard (Python)

```python
import daft

# Cửa hàng TMĐT mẫu (Products + Orders), kiểu Metabase.
products = daft.from_pydict({
    "product_id": [1, 2, 3],
    "product": ["Widget", "Gadget", "Doohickey"],
    "category": ["Gizmos", "Gizmos", "Gadgets"],
    "price": [10.0, 25.0, 5.0],
})
order_ids = list(range(1, 13))
customer_ids = [1, 2, 3, 1, 2, 3, 4, 5, 6, 4, 5, 6]
orders = daft.from_pydict({
    "order_id": order_ids,
    "product_id": [1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3],
    "customer_id": customer_ids,
    "qty": [2, 1, 4, 1, 2, 1, 3, 1, 2, 1, 1, 5],
    "order_month": ["2026-01"] * 4 + ["2026-02"] * 4 + ["2026-03"] * 4,
})

# Dòng doanh thu: join orders với giá sản phẩm.
rev = daft.sql(
    "SELECT o.customer_id, o.qty, o.order_month, p.product, p.category, "
    "(p.price * o.qty) AS revenue "
    "FROM orders o JOIN products p ON o.product_id = p.product_id",
    orders=orders, products=products,
)

# KPI.
total = daft.sql("SELECT SUM(revenue) AS t FROM rev", rev=rev).to_pydict()["t"][0]
top = daft.sql(
    "SELECT product, SUM(revenue) AS r FROM rev GROUP BY product ORDER BY r DESC LIMIT 1",
    rev=rev,
).to_pydict()
print("Tổng doanh thu :", round(float(total), 2))
print("Giá trị đơn TB :", round(float(total) / len(order_ids), 2))
print("Số khách hàng  :", len(set(customer_ids)))
print("SP bán chạy    :", top["product"][0])

# "Biểu đồ" dashboard (render dưới dạng bảng Daft).
display(daft.sql(
    "SELECT category, SUM(revenue) AS revenue FROM rev GROUP BY category ORDER BY revenue DESC",
    rev=rev,
))
display(daft.sql(
    "SELECT order_month, SUM(revenue) AS revenue FROM rev GROUP BY order_month ORDER BY order_month",
    rev=rev,
))
```

## Cell 2 — cùng truy vấn dưới dạng SQL cell Daft-local

Thêm một **SQL cell**, đặt **Target** thành **Daft (local)**, rồi viết SQL thuần —
nó chạy `daft.sql(...)` trên các DataFrame `orders`/`products` mà cell 1 đã bind,
và gán kết quả vào `df`:

```sql
SELECT p.category, SUM(p.price * o.qty) AS revenue
FROM orders o
JOIN products p ON o.product_id = p.product_id
GROUP BY p.category
ORDER BY revenue DESC
```

Kết quả: `Gizmos 195`, `Gadgets 60` — được materialize và hiển thị ngay.

> Lưu ý: `display()` (và SQL cell) materialize kết quả đã cắt bằng `.collect()` để
> render các dòng; nếu để Daft frame ở dạng lazy thì chỉ hiện schema kèm dòng
> "(No data to display: Dataframe not materialized)".
