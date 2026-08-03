/**
 * What this file defends is not "the code compiles" — it is that the DATA is still the data
 * every chart-parity measurement in PRs 01–06 was taken against.
 *
 * The nine `VALUES` literals were recovered from the running `demo` workspace rather than
 * retyped, precisely because a literal that differs by one row silently changes what a future
 * comparison means. Nothing downstream would notice: the seed would succeed, the dashboard
 * would render, and only the numbers would be wrong. So the anchor rows are asserted here.
 *
 * `describe`/`it`/`expect` come from vitest's globals — see the note in `params.test.ts` for
 * why a bare `from "vitest"` cannot resolve from outside `frontend/`.
 */
import { DATASETS, datasetByName, assertMetricsDeclared, assertColumnsMatch } from "./datasets";

const NAMES = [
  "MB Monthly", "MB By Category", "MB Month x Category", "MB Scatter", "MB By State",
  "MB Sankey", "MB Raw Orders", "MB Product Prices", "MB Funnel",
];

describe("the nine datasets", () => {
  it("are all nine, by name", () => {
    expect(DATASETS.map((d) => d.name)).toEqual(NAMES);
  });

  it("each carry SQL, columns, and a description", () => {
    for (const d of DATASETS) {
      expect(d.sql, `${d.name} sql`).toMatch(/^SELECT \* FROM \(VALUES/);
      expect(d.sql, `${d.name} sql`).toMatch(/\) AS t\(/);
      expect(d.columns.length, `${d.name} columns`).toBeGreaterThan(0);
      expect(d.description, `${d.name} description`).not.toBe("");
    }
  });

  it("name every column their own SQL aliases", () => {
    // The `AS t(a, b, c)` tail is the server's only source for column names, so a hand-edited
    // `columns` list that drifts from it would make every chart on that dataset name a column
    // that does not exist.
    for (const d of DATASETS) {
      const alias = /\) AS t\(([^)]*)\)\s*$/.exec(d.sql);
      expect(alias, `${d.name} has no AS t(...) tail`).not.toBeNull();
      const fromSql = alias![1].split(",").map((s) => s.trim());
      expect(d.columns.map((c) => c.column_name), d.name).toEqual(fromSql);
    }
  });
});

describe("the anchor numbers survived recovery", () => {
  // The number the plan names: 2026-01 → 205 orders / 11,094.7. Verified against a live
  // `POST /charts/run` on `MB Monthly` at recovery time; frozen here so a future edit to the
  // literal has to change this test too.
  it("MB Monthly still starts at 2026-01 → 205 orders / 11,094.7", () => {
    const sql = datasetByName("MB Monthly").sql;
    expect(sql).toContain("(CAST(DATE '2026-01-01' AS DATE), CAST(205 AS BIGINT), CAST(11094.7 AS DOUBLE)");
    expect(sql).toContain("(DATE '2026-12-01', 418, 24730.43, 59.16)");
  });

  it("MB By Category still totals the four Sample-DB categories", () => {
    const sql = datasetByName("MB By Category").sql;
    expect(sql).toContain("(CAST('Widget' AS VARCHAR), CAST(5061 AS BIGINT), CAST(423667.5 AS DOUBLE))");
    expect(sql).toContain("('Doohickey', 3976, 297270.99)");
  });

  it("MB Raw Orders is still exactly the first 60 orders", () => {
    const sql = datasetByName("MB Raw Orders").sql;
    // 60 rows, and the last one is order 60 — a truncated paste would still parse.
    expect(sql.split("\n").filter((l) => /^\s*\(/.test(l))).toHaveLength(60);
    expect(sql).toContain("(60, DATE '2027-07-10', 'Small Granite Gloves'");
  });

  it("MB Funnel still has the four steps Metabase's funnel card shows", () => {
    const sql = datasetByName("MB Funnel").sql;
    for (const step of ["Visited", "Added to cart", "Checkout", "Purchased"]) expect(sql).toContain(step);
    expect(sql).toContain("131320");
  });

  it("MB By State still covers 48 states", () => {
    const sql = datasetByName("MB By State").sql;
    expect(sql.split("\n").filter((l) => /^\s*\(/.test(l))).toHaveLength(48);
    expect(sql).toContain("('TX', 1342, 108466.6)");
  });
});

describe("the temporal columns are marked", () => {
  // `is_temporal` is what makes the studio default a date dimension to monthly grouping. If it
  // were dropped, `MB · Line`'s x-axis would silently go back to raw dates — the exact defect
  // PR 03 spent a whole pass on.
  it("marks order_month and created_at, and nothing else", () => {
    const temporal = DATASETS.flatMap((d) =>
      d.columns.filter((c) => c.is_temporal).map((c) => `${d.name}.${c.column_name}`));
    expect(temporal.sort()).toEqual([
      "MB Month x Category.order_month", "MB Monthly.order_month", "MB Raw Orders.created_at",
    ]);
  });
});

describe("assertMetricsDeclared", () => {
  it("passes when every named metric is declared", () => {
    expect(() => assertMetricsDeclared([
      { chart: "MB · Line", dataset: "MB Monthly", metrics: ["revenue"] },
      { chart: "MB · Combo", dataset: "MB Monthly", metrics: ["orders", "revenue"] },
    ])).not.toThrow();
  });

  it("ignores undefined — an unset optional field is not an undeclared metric", () => {
    expect(() => assertMetricsDeclared([
      { chart: "MB · Scatter", dataset: "MB Scatter", metrics: ["rating", undefined] },
    ])).not.toThrow();
  });

  it("throws naming the chart and the metric when one is a bare column", () => {
    // `avg_total` IS a column of MB Monthly and IS declared; `price` is neither.
    expect(() => assertMetricsDeclared([
      { chart: "MB · Line", dataset: "MB Monthly", metrics: ["price"] },
    ])).toThrow(/MB · Line: "price" is not a declared metric of MB Monthly/);
  });

  it("throws for a dataset that declares none at all", () => {
    // MB Raw Orders is columns-only — the table and object charts aggregate nothing.
    expect(() => assertMetricsDeclared([
      { chart: "MB · Table", dataset: "MB Raw Orders", metrics: ["total"] },
    ])).toThrow(/declared: \(none\)/);
  });

  it("reports every problem at once, not just the first", () => {
    try {
      assertMetricsDeclared([
        { chart: "A", dataset: "MB Monthly", metrics: ["nope"] },
        { chart: "B", dataset: "MB Sankey", metrics: ["alsonope"] },
      ]);
      expect.unreachable("should have thrown");
    } catch (e) {
      expect((e as Error).message).toContain("A: \"nope\"");
      expect((e as Error).message).toContain("B: \"alsonope\"");
    }
  });
});

describe("datasetByName", () => {
  it("throws with the list of known names", () => {
    expect(() => datasetByName("MB Nope")).toThrow(/no harness dataset named "MB Nope".*MB Monthly/s);
  });
});

describe("assertColumnsMatch", () => {
  const ds = datasetByName("MB Monthly");

  it("accepts the columns this file declares", () => {
    expect(() => assertColumnsMatch(ds, ds.columns)).not.toThrow();
  });

  it("rejects a changed is_temporal — the flag that decides monthly grouping", () => {
    const drifted = ds.columns.map((c) => ({ ...c, is_temporal: false }));
    expect(() => assertColumnsMatch(ds, drifted)).toThrow(/inferred different columns/);
  });

  it("rejects a changed data type", () => {
    const drifted = ds.columns.map((c, i) => (i === 1 ? { ...c, data_type: "integer" } : c));
    expect(() => assertColumnsMatch(ds, drifted)).toThrow(/inferred different columns/);
  });

  it("rejects a dropped column", () => {
    expect(() => assertColumnsMatch(ds, ds.columns.slice(1))).toThrow(/inferred different columns/);
  });
});
