#!/usr/bin/env python3
"""
Seed / re-init the "E-commerce Insights (egret clone)" dashboard.

Recreates the 22 charts and the full 3-tab dashboard (Overview / Portfolio Performance /
Website Analysis), mirroring Metabase's sample "E-commerce Insights" dashboard, and the four
datasets it reads (ecom_orders, ecom_reviews, accounts, funnel_steps).

The datasets are virtual datasets over the TPC-H sample data that a Trino serves (`tpch.tiny`),
reshaped into an e-commerce store: orders carry a product category, a vendor, a US state, a
marketing source, an age group and a rating, dates are shifted by 31 years so they run to
mid-2029, and totals are scaled to dollars. The values are illustrative, not real sales.
Idempotent: datasets and metrics are created when missing; charts with the names below are
deleted and recreated; the dashboard of the same name is reused and its cards replaced.

Defaults match the Docker demo stack (`just demo-up`, database `trino-demo`, workspace `default`).
Everything is overridable by environment variables:

    BASE_URL=http://egret.localtest.me:8080 EGRET_WS=default EGRET_DB=trino-demo \
    EGRET_EMAIL=admin@egret.local EGRET_PASSWORD=... python3 scripts/seed_ecommerce_clone.py

(or EGRET_COOKIES="name=value; ..." and EGRET_CSRF=<egret_csrf value> to reuse a signed-in session).

deploy/demo/seed.sh runs it as part of `just demo-up` / `just demo-seed`.
"""
import json
import os
import sys
import urllib.error
import urllib.request

BASE_URL = os.environ.get("BASE_URL", "http://localhost:8080").rstrip("/")
EMAIL = os.environ.get("EGRET_EMAIL", "admin@egret.local")
PASSWORD = os.environ.get("EGRET_PASSWORD", "Admin12345!")
WS = os.environ.get("EGRET_WS", "default")
DB_NAME = os.environ.get("EGRET_DB", "trino-demo")
DASH_NAME = "E-commerce Insights (egret clone)"


class Api:
    def __init__(self):
        self.cookies = ""
        self.csrf = ""

    def _req(self, method, path, body=None):
        headers = {"Content-Type": "application/json"}
        if self.cookies:
            headers["Cookie"] = self.cookies
        if self.csrf:
            headers["X-Egret-CSRF"] = self.csrf
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(BASE_URL + path, data=data, headers=headers, method=method)
        try:
            resp = urllib.request.urlopen(req, timeout=180)
        except urllib.error.HTTPError as e:
            sys.exit(f"{method} {path} -> {e.code} {e.read().decode()[:400]}")
        for sc in resp.headers.get_all("Set-Cookie") or []:
            pair = sc.split(";")[0]
            name = pair.split("=", 1)[0]
            # a later Set-Cookie for the same name replaces the earlier one
            self.cookies = "; ".join([c for c in self.cookies.split("; ") if c and not c.startswith(name + "=")] + [pair])
            if name.lower() == "egret_csrf":
                self.csrf = pair.split("=", 1)[1]
        raw = resp.read().decode()
        return json.loads(raw) if raw else {}

    def login(self):
        # A caller that is already signed in (deploy/demo/seed.sh) passes its session, so the seed
        # does not spend a second sign-in from the login rate limit.
        if os.environ.get("EGRET_COOKIES") and os.environ.get("EGRET_CSRF"):
            self.cookies = os.environ["EGRET_COOKIES"].rstrip("; ")
            self.csrf = os.environ["EGRET_CSRF"]
            return
        self._req("POST", "/api/auth/login", {"email": EMAIL, "password": PASSWORD})

    def get(self, path):
        return self._req("GET", path)

    def post(self, path, body):
        return self._req("POST", path, body)

    def put(self, path, body):
        return self._req("PUT", path, body)

    def delete(self, path):
        return self._req("DELETE", path)


def items(x):
    return x if isinstance(x, list) else x.get("items", [])


# --------------------------------------------------------------------------------------------------
# Datasets
# --------------------------------------------------------------------------------------------------
STATES = ("AL AK AZ AR CA CO CT DE FL GA HI ID IL IN IA KS KY LA ME MD MA MI MN MS MO MT NE NV NH NJ NM NY "
          "NC ND OH OK OR PA RI SC SD TN TX UT VT VA WA WV WI WY").split()
assert len(STATES) == 50


def arr(values):
    return "ARRAY[" + ",".join("'" + v + "'" for v in values) + "]"


def pick(values, key):
    return f"element_at({arr(values)}, 1 + CAST(mod({key}, {len(values)}) AS integer))"


CATEGORY = pick(["Doohickey", "Gadget", "Gizmo", "Widget"], "p.partkey")
# Ratings differ by category (means about 3.1, 3.9, 4.5 and 3.5, in the order of CATEGORY), so the
# satisfaction chart has something to say.
RATING = ("CAST(element_at(CASE mod(p.partkey, 4) "
          "WHEN 0 THEN ARRAY[1,2,2,3,3,3,4,4,4,5] WHEN 1 THEN ARRAY[2,3,3,4,4,4,4,5,5,5] "
          "WHEN 2 THEN ARRAY[3,4,4,4,5,5,5,5,5,5] ELSE ARRAY[2,2,3,3,3,4,4,4,5,5] END, "
          "1 + CAST(mod(o.orderkey, 10) AS integer)) AS integer)")
SOURCE = pick(["Google", "Facebook", "Twitter", "Organic", "Affiliate"], "c.custkey")

ORDERS_SQL = f"""SELECT o.orderkey AS id,
       date_add('year', 31, o.orderdate) AS created_at,
       date_trunc('quarter', date_add('year', 31, o.orderdate)) AS order_quarter,
       round(o.totalprice / 1000, 2) AS total,
       l.discount AS discount,
       {RATING} AS rating,
       {CATEGORY} AS category,
       replace(p.mfgr, 'Manufacturer#', 'Vendor ') AS vendor,
       p.brand || ' ' || split_part(p.type, ' ', 3) AS product,
       {pick(STATES, "c.custkey")} AS state,
       {SOURCE} AS source,
       {pick(["18-24", "25-34", "35-44", "45-54", "55-64", "65+"], "(c.custkey + c.custkey / 3)")} AS age_group
FROM tpch.tiny.orders o
JOIN tpch.tiny.customer c ON o.custkey = c.custkey
JOIN (SELECT orderkey, min(partkey) AS partkey, avg(discount) AS discount
      FROM tpch.tiny.lineitem GROUP BY orderkey) l ON l.orderkey = o.orderkey
JOIN tpch.tiny.part p ON p.partkey = l.partkey
WHERE o.orderdate < DATE '1998-07-01'"""

REVIEWS_SQL = f"""SELECT o.orderkey AS id,
       replace(c.name, 'Customer#', 'Customer ') AS reviewer,
       {RATING} AS rating,
       p.brand || ' ' || split_part(p.type, ' ', 3) AS product,
       {CATEGORY} AS category,
       substr(o.comment, 1, 90) AS body
FROM tpch.tiny.orders o
JOIN tpch.tiny.customer c ON o.custkey = c.custkey
JOIN (SELECT orderkey, min(partkey) AS partkey FROM tpch.tiny.lineitem GROUP BY orderkey) l ON l.orderkey = o.orderkey
JOIN tpch.tiny.part p ON p.partkey = l.partkey
WHERE o.orderkey <= 600"""

ACCOUNTS_SQL = f"""SELECT c.custkey AS id,
       {pick(["Basic", "Business", "Premium", "Enterprise"], "c.custkey")} AS plan,
       {SOURCE} AS source,
       CAST(1 + mod(c.custkey, 25) AS integer) AS seats,
       n.name AS country,
       date_add('day', CAST(1640 * sqrt(c.custkey / 1500.0) AS integer), DATE '2025-01-01') AS created_at,
       date_trunc('month', date_add('day', CAST(1640 * sqrt(c.custkey / 1500.0) AS integer), DATE '2025-01-01')) AS acct_month
FROM tpch.tiny.customer c
JOIN tpch.tiny.nation n ON c.nationkey = n.nationkey"""

FUNNEL_SQL = """SELECT * FROM (VALUES (1, 'Visited site', 12400), (2, 'Viewed product', 8100),
       (3, 'Added to cart', 3900), (4, 'Started checkout', 2300), (5, 'Paid', 1500)) AS t(step, stage, users)"""


def m(name, verbose, expression):
    return {"metric_name": name, "verbose_name": verbose, "expression": expression}


DATASETS = {
    "ecom_orders": ("Orders of an illustrative online store, built from the TPC-H sample data.", ORDERS_SQL,
                    [m("revenue", "Revenue", "round(sum(total), 0)"), m("orders", "Orders", "count(*)"),
                     m("avg_total", "Average order value", "round(avg(total), 2)"),
                     m("avg_discount", "Average discount", "avg(discount)"),
                     m("avg_rating", "Average rating", "avg(rating)")]),
    "ecom_reviews": ("Customer reviews for the store's products.", REVIEWS_SQL,
                     [m("avg_rating", "Average rating", "avg(rating)")]),
    "accounts": ("Subscription accounts of the store's website.", ACCOUNTS_SQL,
                 [m("acct_count", "Subscriptions", "count(*)"), m("seats_sum", "Seats", "sum(seats)")]),
    "funnel_steps": ("Visitors per checkout stage (illustrative values).", FUNNEL_SQL,
                     [m("users", "Users", "sum(users)")]),
}


def ensure_datasets(api):
    db = next((d for d in items(api.get(f"/api/ws/{WS}/databases")) if d["name"] == DB_NAME), None)
    if db is None:
        sys.exit(f"database {DB_NAME!r} not found in workspace {WS!r}: set EGRET_DB or run deploy/demo/seed.sh first")
    have = {d["name"]: d["id"] for d in items(api.get(f"/api/ws/{WS}/datasets"))}
    ids = {}
    for name, (description, sql, metrics) in DATASETS.items():
        if name not in have:
            have[name] = api.post(f"/api/ws/{WS}/datasets", {
                "name": name, "database_id": db["id"], "description": description,
                "source": {"kind": "virtual", "sql": sql}})["id"]
            print(f"  + dataset {name}")
        else:  # keep an existing dataset in step with this script
            api.put(f"/api/ws/{WS}/datasets/{have[name]}", {"name": name, "description": description, "sql": sql})
        existing = {x["metric_name"]: x for x in api.get(f"/api/ws/{WS}/datasets/{have[name]}")["metrics"]}
        for metric in metrics:
            old = existing.get(metric["metric_name"])
            if old is None:
                api.post(f"/api/ws/{WS}/datasets/{have[name]}/metrics", metric)
            elif old["expression"] != metric["expression"]:
                api.put(f"/api/ws/{WS}/datasets/{have[name]}/metrics/{old['id']}", metric)
        ids[name] = have[name]
    return ids


# --------------------------------------------------------------------------------------------------
# Charts
# --------------------------------------------------------------------------------------------------
def spec(metrics=None, dimensions=None, columns=None, orderby=None, rollup=None, limit=1000,
         temporal=None, filters=None):
    s = {
        "metrics": metrics or [],
        "dimensions": dimensions or [],
        "columns": columns or [],
        "filters": filters or [],
        "orderby": orderby or [],
        "rollup_groups": rollup or [],
        "row_limit": limit,
    }
    if temporal:
        s["temporal_units"] = temporal
    return s


# The orders run to the end of June 2029; "this quarter" scopes the KPI cards to the last one
# (it matches the "Revenue per quarter" smartscalar's current period).
THIS_QUARTER = [{"col": "created_at", "op": "ge", "value": "2029-04-01"}]


def ob(field, desc=False):
    return {"field": field, "desc": desc}


def settings(viz, p, columns):
    """Flat v2 settings (`<viz>.<key>`) from the short per-viz dict used in the chart list below."""
    c = "cartesian."
    if viz in ("bar", "line", "area", "row", "combo", "scatter"):
        out = {c + "xAxis": p["xAxis"], c + "yMetrics": p["yMetrics"]}
        for k in ("series", "xMetric", "rightAxisMetrics", "seriesTypes", "stacked", "showLegend", "bubbleMetric"):
            if k in p:
                out[c + k] = p[k]
        return out
    if viz == "pie":
        return {"pie.dimensions": [p["dimension"]], "pie.metric": p["metric"], "pie.donut": True,
                "pie.showLegend": p.get("showLegend", True)}
    if viz in ("scalar", "progress", "gauge", "smartscalar", "boxplot", "funnel", "waterfall", "sankey"):
        out = {f"{viz}.{k}": v for k, v in p.items() if k not in ("min", "max")}
        if viz == "gauge":
            out["gauge.segments"] = [{"min": 0, "max": 2.5, "color": "#ef4444", "label": "Poor"},
                                     {"min": 2.5, "max": 4, "color": "#f59e0b", "label": "Fair"},
                                     {"min": 4, "max": 5, "color": "#22c55e", "label": "Good"}]
        if viz == "smartscalar":
            out["smartscalar.comparisons"] = [{"id": "c1", "type": "previousPeriod"}]
        return out
    if viz == "pivot_table":
        return {"pivot.rows": p["rows"], "pivot.cols": p["cols"], "pivot.metrics": p["metrics"]}
    if viz == "map":
        return {"map.key": "us_states", "map.type": "region", "map.region": p["region"], "map.metric": p["metric"]}
    if viz in ("table", "object"):
        return {f"{viz}.columns": columns}
    return {}


# Each chart: (name, dataset, viz_type, short settings, spec, tab, col, row, size_x, size_y)
def charts():
    return [
        # ----- Tab 0: Overview -----
        ("Revenue per quarter", "ecom_orders", "smartscalar", {"dimension": "order_quarter", "metric": "revenue"},
         spec(["revenue"], ["order_quarter"], orderby=[ob("order_quarter")]), 0, 0, 2, 8, 4),
        ("Revenue goal for this quarter", "ecom_orders", "progress", {"metric": "revenue", "goal": 90000},
         spec(["revenue"], [], limit=1, filters=THIS_QUARTER), 0, 8, 2, 9, 4),
        ("Total orders this quarter", "ecom_orders", "scalar", {"metric": "orders"},
         spec(["orders"], [], limit=1, filters=THIS_QUARTER), 0, 17, 2, 7, 4),
        ("Revenue and orders over time", "ecom_orders", "combo",
         {"xAxis": "created_at", "yMetrics": ["revenue", "orders"], "rightAxisMetrics": ["orders"],
          "seriesTypes": {"revenue": "bar", "orders": "line"}},
         spec(["revenue", "orders"], ["created_at"], orderby=[ob("created_at")], temporal={"created_at": "month"}),
         0, 0, 6, 24, 8),
        ("Total orders by product category", "ecom_orders", "pie",
         {"dimension": "category", "metric": "orders", "showLegend": True}, spec(["orders"], ["category"]),
         0, 0, 16, 12, 7),
        ("Revenue by product category", "ecom_orders", "bar", {"xAxis": "category", "yMetrics": ["revenue"]},
         spec(["revenue"], ["category"], orderby=[ob("revenue", True)]), 0, 12, 16, 12, 7),
        ("Orders by product category", "ecom_orders", "line",
         {"xAxis": "created_at", "yMetrics": ["orders"], "series": "category"},
         spec(["orders"], ["created_at", "category"], orderby=[ob("created_at")], temporal={"created_at": "quarter"},
              limit=5000), 0, 0, 23, 24, 7),
        ("Orders according to sources per quarter", "ecom_orders", "pivot_table",
         {"rows": ["source"], "cols": ["order_quarter"], "metrics": ["orders"]},
         spec(["orders"], ["source", "order_quarter"], rollup=[["source"], ["order_quarter"]], limit=5000),
         0, 0, 30, 24, 8),

        # ----- Tab 1: Portfolio Performance -----
        ("Product breakdown", "ecom_orders", "pie", {"dimension": "vendor", "metric": "orders", "showLegend": True},
         spec(["orders"], ["vendor"]), 1, 0, 2, 12, 8),
        ("Order value distribution by category", "ecom_orders", "boxplot", {"category": "category", "value": "total"},
         spec([], [], columns=["category", "total"], limit=5000), 1, 12, 2, 12, 8),
        ("Best selling products", "ecom_orders", "row", {"xAxis": "product", "yMetrics": ["orders"]},
         spec(["orders"], ["product"], orderby=[ob("orders", True)], limit=10), 1, 0, 10, 12, 8),
        ("Revenue by state", "ecom_orders", "map", {"region": "state", "metric": "revenue"},
         spec(["revenue"], ["state"]), 1, 12, 10, 12, 8),
        ("Order amount vs. order volume by product", "ecom_orders", "scatter",
         {"xAxis": "product", "xMetric": "orders", "yMetrics": ["avg_total"]},
         spec(["orders", "avg_total"], ["product"]), 1, 0, 18, 24, 8),
        ("Product category orders per age", "ecom_orders", "bar",
         {"xAxis": "age_group", "yMetrics": ["orders"], "series": "category"},
         spec(["orders"], ["age_group", "category"], orderby=[ob("age_group")], limit=5000), 1, 0, 26, 24, 6),
        ("Average product rating", "ecom_orders", "gauge", {"metric": "avg_rating", "min": 0, "max": 5},
         spec(["avg_rating"], [], limit=1), 1, 0, 34, 10, 7),
        ("Customer satisfaction per category", "ecom_reviews", "bar",
         {"xAxis": "category", "yMetrics": ["avg_rating"]}, spec(["avg_rating"], ["category"]), 1, 10, 34, 14, 7),
        ("Customer survey responses", "ecom_reviews", "table", {},
         spec([], [], columns=["reviewer", "rating", "product", "category", "body"], limit=100), 1, 0, 41, 24, 10),

        # ----- Tab 2: Website Analysis -----
        ("User flow diagram", "accounts", "sankey", {"source": "source", "target": "plan", "metric": "acct_count"},
         spec(["acct_count"], ["source", "plan"]), 2, 0, 2, 24, 7),
        ("Checkout funnel", "funnel_steps", "funnel", {"dimension": "stage", "metric": "users"},
         spec(["users"], ["stage"], orderby=[ob("users", True)]), 2, 0, 9, 14, 7),
        ("Number of subscriptions", "accounts", "area", {"xAxis": "acct_month", "yMetrics": ["acct_count"]},
         spec(["acct_count"], ["acct_month"], orderby=[ob("acct_month")]), 2, 14, 9, 10, 7),
        ("Subscription seats over time", "accounts", "waterfall", {"xAxis": "acct_month", "metric": "seats_sum"},
         spec(["seats_sum"], ["acct_month"], orderby=[ob("acct_month")]), 2, 0, 16, 14, 8),
        ("Most recent subscription", "accounts", "object", {},
         spec([], [], columns=["id", "plan", "source", "seats", "country", "created_at"],
              orderby=[ob("created_at", True)], limit=20), 2, 14, 16, 10, 8),
    ]


def text_cards():
    H = lambda t: {"card_type": "heading", "options": {"text": t}}
    T = lambda t: {"card_type": "text", "options": {"text": t}}
    # (card, tab, col, row, size_x, size_y)
    return [
        (H("Overall Business Health"), 0, 0, 0, 24, 1),
        (T("Revenue and orders **per quarter**, with this quarter's goal and order count"), 0, 0, 1, 24, 1),
        (H("Deeper Dive"), 0, 0, 14, 24, 1),
        (T("How the four product categories compare, by orders, revenue and over time"), 0, 0, 15, 24, 1),
        (H("Top Performing Products"), 1, 0, 0, 24, 1),
        (T("Find below a breakdown of products and their performance across categories, states, and customer demographics"), 1, 0, 1, 24, 1),
        (H("Customer Satisfaction"), 1, 0, 32, 24, 1),
        (T("Customer ratings for our products"), 1, 0, 33, 24, 1),
        (H("Website Analysis"), 2, 0, 0, 24, 1),
        (T("Take a look into our customer funnel and website engagement"), 2, 0, 1, 24, 1),
    ]


TABS = [
    {"name": "Overview", "position": 0},
    {"name": "Portfolio Performance", "position": 1},
    {"name": "Website Analysis", "position": 2},
]


def main():
    api = Api()
    api.login()
    print("logged in")
    ds_id = ensure_datasets(api)

    chart_defs = charts()
    names = {c[0] for c in chart_defs}

    # Idempotency: delete existing charts whose names we are about to recreate.
    existing = items(api.get(f"/api/ws/{WS}/charts"))
    for c in existing:
        if c.get("name") in names:
            api.delete(f"/api/ws/{WS}/charts/{c['id']}")
    print(f"cleared {sum(1 for c in existing if c.get('name') in names)} existing charts")

    # Create charts.
    name_to_id = {}
    for (name, ds, viz, short, sp, *_pos) in chart_defs:
        params = {"version": 2, "settings": settings(viz, short, sp["columns"]), "spec": sp}
        res = api.post(f"/api/ws/{WS}/charts", {"dataset_id": ds_id[ds], "name": name, "viz_type": viz,
                                                "params": params})
        cid = res.get("id")
        if not cid:
            print(f"  ! FAILED to create '{name}': {res}")
            continue
        name_to_id[name] = cid
        print(f"  + {viz:12} {name}")

    # Build the full card list (text/heading + charts).
    cards = []
    for (card, tab, col, row, sx, sy) in text_cards():
        cards.append({**card, "grid_col": col, "grid_row": row, "size_x": sx, "size_y": sy, "tab_index": tab})
    for (name, ds, viz, short, sp, tab, col, row, sx, sy) in chart_defs:
        if name not in name_to_id:
            continue
        cards.append({"chart_id": name_to_id[name], "card_type": "chart",
                      "grid_col": col, "grid_row": row, "size_x": sx, "size_y": sy,
                      "tab_index": tab, "options": {}})

    # Find-or-create the dashboard.
    dash = next((d for d in items(api.get(f"/api/ws/{WS}/dashboards")) if d.get("name") == DASH_NAME), None)
    did = dash["id"] if dash else api.post(f"/api/ws/{WS}/dashboards", {"name": DASH_NAME, "description": ""})["id"]
    api.put(f"/api/ws/{WS}/dashboards/{did}", {
        "name": DASH_NAME,
        "description": "Revenue, orders, products, customers and website traffic of an illustrative online store.",
        "width": "full", "tabs": TABS, "cards": cards, "parameters": [],
    })
    print(f"\ndashboard '{DASH_NAME}'  id={did}")
    print(f"  {len(name_to_id)}/{len(chart_defs)} charts, {len(cards)} cards across {len(TABS)} tabs")


if __name__ == "__main__":
    main()
