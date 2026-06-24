#!/usr/bin/env python3
"""
Seed / re-init the "E-commerce Insights (egret clone)" dashboard.

Recreates the 22 charts and the full 3-tab dashboard (Overview / Portfolio Performance /
Website Analysis) from the existing `eng`-workspace datasets, mirroring Metabase's sample
"E-commerce Insights" dashboard. Idempotent: deletes any charts whose names match before
recreating, and reuses the dashboard of the same name if it already exists.

Prereqs: the egret dev backend is up on BASE_URL and the datasets below already exist
(ecom_orders, ecom_reviews, accounts, funnel_steps). Run:

    python3 scripts/seed_ecommerce_clone.py

All Metabase viz types now have a native egret equivalent (US map, object detail, …).
"""
import json
import sys
import urllib.request

BASE_URL = "http://localhost:8080"
EMAIL = "admin@egret.local"
PASSWORD = "Admin12345!"
WS = "eng"
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
        resp = urllib.request.urlopen(req)
        set_cookie = resp.headers.get_all("Set-Cookie") or []
        for sc in set_cookie:
            pair = sc.split(";")[0]
            self.cookies = (self.cookies + "; " + pair) if self.cookies else pair
            if pair.lower().startswith("egret_csrf="):
                self.csrf = pair.split("=", 1)[1]
        raw = resp.read().decode()
        return json.loads(raw) if raw else {}

    def login(self):
        self._req("POST", "/api/auth/login", {"email": EMAIL, "password": PASSWORD})

    def get(self, path):
        return self._req("GET", path)

    def post(self, path, body):
        return self._req("POST", path, body)

    def put(self, path, body):
        return self._req("PUT", path, body)

    def delete(self, path):
        return self._req("DELETE", path)


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


# The seeded ecom_orders data runs through Q2 2029; "this quarter" scopes the KPI cards to it
# (matches the "Revenue per quarter" smartscalar's current period).
THIS_QUARTER = [{"col": "created_at", "op": "ge", "value": "2029-04-01"}]


def ob(field, desc=False):
    return {"field": field, "desc": desc}


# Each chart: (name, dataset, viz_type, params, tab, col, row, size_x, size_y)
def charts():
    return [
        # ----- Tab 0: Overview -----
        ("Revenue per quarter", "ecom_orders", "smartscalar",
         {"smartscalar": {"dimension": "order_quarter", "metric": "revenue"},
          "spec": spec(["revenue"], ["order_quarter"], orderby=[ob("order_quarter")])},
         0, 0, 2, 8, 4),
        ("Revenue goal for this quarter", "ecom_orders", "progress",
         {"progress": {"metric": "revenue", "goal": 50000},
          "spec": spec(["revenue"], [], limit=1, filters=THIS_QUARTER)},
         0, 8, 2, 9, 4),
        ("Total orders this quarter", "ecom_orders", "scalar",
         {"scalar": {"metric": "orders"}, "spec": spec(["orders"], [], limit=1, filters=THIS_QUARTER)},
         0, 17, 2, 7, 4),
        ("Revenue and orders over time", "ecom_orders", "combo",
         {"cartesian": {"xAxis": "created_at", "yMetrics": ["revenue"], "rightAxisMetrics": ["orders"],
                        "seriesTypes": {"revenue": "bar", "orders": "line"}},
          "spec": spec(["revenue", "orders"], ["created_at"], orderby=[ob("created_at")],
                       temporal={"created_at": "month"})},
         0, 0, 6, 24, 8),
        ("Total orders by product category", "ecom_orders", "pie",
         {"pie": {"dimension": "category", "metric": "orders", "showLegend": True},
          "spec": spec(["orders"], ["category"])},
         0, 0, 16, 12, 7),
        ("Revenue by product category", "ecom_orders", "bar",
         {"cartesian": {"xAxis": "category", "yMetrics": ["revenue"]},
          "spec": spec(["revenue"], ["category"], orderby=[ob("revenue", True)])},
         0, 12, 16, 12, 7),
        ("Orders by product category", "ecom_orders", "line",
         {"cartesian": {"xAxis": "created_at", "yMetrics": ["orders"], "series": "category"},
          "spec": spec(["orders"], ["created_at", "category"], orderby=[ob("created_at")],
                       temporal={"created_at": "month"}, limit=5000)},
         0, 0, 23, 24, 7),
        ("Orders according to sources per quarter", "ecom_orders", "pivot_table",
         {"rows": ["source"], "cols": ["order_quarter"], "metrics": ["orders"],
          "spec": spec(["orders"], ["source", "order_quarter"],
                       rollup=[["source"], ["order_quarter"]], limit=5000)},
         0, 0, 30, 24, 8),

        # ----- Tab 1: Portfolio Performance -----
        ("Product breakdown", "ecom_orders", "pie",
         {"pie": {"dimension": "vendor", "metric": "orders", "showLegend": True},
          "spec": spec(["orders"], ["vendor"])},
         1, 0, 2, 12, 8),
        ("Order value distribution by category", "ecom_orders", "boxplot",
         {"boxplot": {"category": "category", "value": "total"},
          "spec": spec([], [], columns=["category", "total"], limit=5000)},
         1, 12, 2, 12, 8),
        ("Best selling products", "ecom_orders", "row",
         {"cartesian": {"xAxis": "product", "yMetrics": ["orders"]},
          "spec": spec(["orders"], ["product"], orderby=[ob("orders", True)])},
         1, 0, 10, 12, 8),
        ("Revenue by state", "ecom_orders", "map",
         {"map": {"mapKey": "us_states", "region": "state", "metric": "revenue"},
          "spec": spec(["revenue"], ["state"])},
         1, 12, 10, 12, 8),
        ("Total order amount vs. discount given", "ecom_orders", "scatter",
         {"cartesian": {"xAxis": "product", "xMetric": "avg_discount", "yMetrics": ["avg_total"]},
          "spec": spec(["avg_discount", "avg_total"], ["product"])},
         1, 0, 18, 24, 8),
        ("Product category orders per age", "ecom_orders", "bar",
         {"cartesian": {"xAxis": "age_group", "yMetrics": ["orders"], "series": "category"},
          "spec": spec(["orders"], ["age_group", "category"], limit=5000)},
         1, 0, 26, 24, 6),
        ("Average product rating", "ecom_orders", "gauge",
         {"gauge": {"metric": "avg_rating", "min": 0, "max": 5},
          "spec": spec(["avg_rating"], [], limit=1)},
         1, 0, 34, 10, 7),
        ("Customer satisfaction per category", "ecom_reviews", "bar",
         {"cartesian": {"xAxis": "category", "yMetrics": ["avg_rating"]},
          "spec": spec(["avg_rating"], ["category"])},
         1, 10, 34, 14, 7),
        ("Customer survey responses", "ecom_reviews", "table",
         {"spec": spec([], [], columns=["reviewer", "rating", "product", "category", "body"])},
         1, 0, 41, 24, 10),

        # ----- Tab 2: Website Analysis -----
        ("User flow diagram", "accounts", "sankey",
         {"sankey": {"source": "source", "target": "plan", "metric": "acct_count"},
          "spec": spec(["acct_count"], ["source", "plan"])},
         2, 0, 2, 24, 7),
        ("Checkout funnel", "funnel_steps", "funnel",
         {"funnel": {"dimension": "stage", "metric": "users"},
          "spec": spec(["users"], ["stage"], orderby=[ob("users", True)])},
         2, 0, 9, 14, 7),
        ("Number of subscriptions", "accounts", "area",
         {"cartesian": {"xAxis": "acct_month", "yMetrics": ["acct_count"]},
          "spec": spec(["acct_count"], ["acct_month"], orderby=[ob("acct_month")])},
         2, 14, 9, 10, 7),
        ("Subscription seats over time", "accounts", "waterfall",
         {"waterfall": {"xAxis": "acct_month", "metric": "seats_sum"},
          "spec": spec(["seats_sum"], ["acct_month"], orderby=[ob("acct_month")])},
         2, 0, 16, 14, 7),
        ("Most recent subscription", "accounts", "object",
         {"spec": spec([], [], columns=["id", "plan", "source", "seats", "country", "created_at"],
                       orderby=[ob("created_at", True)], limit=20)},
         2, 14, 16, 10, 6),
    ]


def text_cards():
    H = lambda t: {"card_type": "heading", "options": {"text": t}}
    T = lambda t: {"card_type": "text", "options": {"text": t}}
    # (card, tab, col, row, size_x, size_y)
    return [
        (H("Overall Business Health"), 0, 0, 0, 24, 1),
        (T("Business KPIs *look strong* based on the consistent MoM revenue growth following the growth in the number of orders"), 0, 0, 1, 24, 1),
        (H("Deeper Dive"), 0, 0, 14, 24, 1),
        (T("Widget and Gizmo are our leading product categories"), 0, 0, 15, 24, 1),
        (H("Top Performing Products"), 1, 0, 0, 24, 1),
        (T("Find below a breakdown of products and their performance across categories, states, and customer demographics"), 1, 0, 1, 24, 1),
        (H("Customer Satisfaction"), 1, 0, 32, 24, 1),
        (T("Super important customer ratings for our products"), 1, 0, 33, 24, 1),
        (H("Website Analysis"), 2, 0, 0, 24, 1),
        (T("Take a look into our customer funnel and website engagement"), 2, 0, 1, 24, 1),
        (H("Appendix"), 2, 0, 23, 24, 1),
        (T("Find more guides and tutorials on Metabase, and learn how this dashboard was built"), 2, 0, 24, 24, 1),
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

    # Resolve dataset ids by name.
    ds_list = api.get(f"/api/ws/{WS}/datasets")
    ds_list = ds_list if isinstance(ds_list, list) else ds_list.get("items", [])
    ds_id = {d["name"]: d["id"] for d in ds_list}
    needed = {c[1] for c in charts()}
    missing = needed - set(ds_id)
    if missing:
        sys.exit(f"missing datasets: {sorted(missing)} — create them before seeding")

    chart_defs = charts()
    names = {c[0] for c in chart_defs}

    # Idempotency: delete existing charts whose names we are about to recreate.
    existing = api.get(f"/api/ws/{WS}/charts")
    existing = existing if isinstance(existing, list) else existing.get("items", [])
    for c in existing:
        if c.get("name") in names:
            api.delete(f"/api/ws/{WS}/charts/{c['id']}")
    print(f"cleared {sum(1 for c in existing if c.get('name') in names)} existing charts")

    # Create charts.
    name_to_id = {}
    for (name, ds, viz, params, *_pos) in chart_defs:
        res = api.post(f"/api/ws/{WS}/charts", {
            "dataset_id": ds_id[ds], "name": name, "viz_type": viz, "params": params,
        })
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
    for (name, ds, viz, params, tab, col, row, sx, sy) in chart_defs:
        if name not in name_to_id:
            continue
        cards.append({"chart_id": name_to_id[name], "card_type": "chart",
                      "grid_col": col, "grid_row": row, "size_x": sx, "size_y": sy,
                      "tab_index": tab, "options": {}})

    # Find-or-create the dashboard.
    dashes = api.get(f"/api/ws/{WS}/dashboards")
    dashes = dashes if isinstance(dashes, list) else dashes.get("items", [])
    dash = next((d for d in dashes if d.get("name") == DASH_NAME), None)
    if dash:
        did = dash["id"]
    else:
        did = api.post(f"/api/ws/{WS}/dashboards", {"name": DASH_NAME, "description": ""})["id"]

    api.put(f"/api/ws/{WS}/dashboards/{did}", {
        "name": DASH_NAME, "description": "", "width": "full",
        "tabs": TABS, "cards": cards, "parameters": [],
    })
    print(f"\ndashboard '{DASH_NAME}'  id={did}")
    print(f"  {len(name_to_id)}/{len(chart_defs)} charts, {len(cards)} cards across {len(TABS)} tabs")


if __name__ == "__main__":
    main()
