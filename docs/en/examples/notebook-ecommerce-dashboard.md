# Example: Ecommerce dashboard notebook

A self-contained, copy-pasteable notebook that reproduces a Metabase-style
ecommerce "dashboard" — revenue KPIs, top products, and revenue broken down by
category and month — using only the kernel (Daft). It needs **no engine,
warehouse, or seeded data**, so it runs in any workspace and doubles as a smoke
test for the notebook (see `frontend/tests/e2e/notebook-ecommerce.spec.ts`).

It exercises the whole ad-hoc-analysis path: Python execution, `daft.from_pydict`,
`daft.sql(...)` over bound DataFrames (JOIN / SUM / GROUP BY / ORDER BY),
`display()` table rendering, and the `%%egret_sql --target daft` cell magic.

## The dataset

A tiny store: three products across two categories, twelve orders over three
months. The figures below are deterministic:

| Metric | Value |
| --- | --- |
| Total revenue (Σ price·qty) | **255.0** |
| Average order value (255 / 12) | **21.25** |
| Distinct customers | **6** |
| Top product | **Gadget** (125) |
| Revenue by category | Gizmos **195**, Gadgets **60** |
| Revenue by month | 2026-01 **75**, 2026-02 **110**, 2026-03 **70** |

## Cell 1 — build the data + dashboard KPIs (Python)

```python
import daft

# Sample ecommerce store (Products + Orders), Metabase-style.
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

# Revenue line items: join orders to product prices.
rev = daft.sql(
    "SELECT o.customer_id, o.qty, o.order_month, p.product, p.category, "
    "(p.price * o.qty) AS revenue "
    "FROM orders o JOIN products p ON o.product_id = p.product_id",
    orders=orders, products=products,
)

# KPIs.
total = daft.sql("SELECT SUM(revenue) AS t FROM rev", rev=rev).to_pydict()["t"][0]
top = daft.sql(
    "SELECT product, SUM(revenue) AS r FROM rev GROUP BY product ORDER BY r DESC LIMIT 1",
    rev=rev,
).to_pydict()
print("Total revenue :", round(float(total), 2))
print("Avg order val :", round(float(total) / len(order_ids), 2))
print("Customers     :", len(set(customer_ids)))
print("Top product   :", top["product"][0])

# Dashboard "charts" (rendered as Daft tables).
display(daft.sql(
    "SELECT category, SUM(revenue) AS revenue FROM rev GROUP BY category ORDER BY revenue DESC",
    rev=rev,
))
display(daft.sql(
    "SELECT order_month, SUM(revenue) AS revenue FROM rev GROUP BY order_month ORDER BY order_month",
    rev=rev,
))
```

## Cell 2 — the same query as a Daft-local SQL cell

Add a **SQL cell**, set its **Target** to **Daft (local)**, and write plain SQL —
it runs `daft.sql(...)` over the `orders`/`products` DataFrames bound by cell 1
and binds the result to `df`:

```sql
SELECT p.category, SUM(p.price * o.qty) AS revenue
FROM orders o
JOIN products p ON o.product_id = p.product_id
GROUP BY p.category
ORDER BY revenue DESC
```

Result: `Gizmos 195`, `Gadgets 60` — materialized and shown inline.

> Note: `display()` (and the SQL cell) materialize the capped result with
> `.collect()` so the rows render; a lazy Daft frame would otherwise show only
> its schema plus "(No data to display: Dataframe not materialized)".
