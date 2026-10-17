import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useParams, useSearchParams } from "react-router-dom";
import { parseDottedPath, pathToUrl, urlToPath, type ObjectPath } from "@/routes/governance/data/objectPath";

/**
 * The round trip goes through a REAL router, not through `urlToPath` alone:
 * what decides whether a `/` or a `%` in a name survives is how react-router
 * splits and decodes the pathname into params, and a test that fed
 * `urlToPath` hand-made params would never meet it.
 */
function Probe() {
  const params = useParams();
  const [search] = useSearchParams();
  return <output data-testid="path">{JSON.stringify(urlToPath(params, search))}</output>;
}

function roundTrip(p: ObjectPath, tab?: string): ObjectPath {
  const url = pathToUrl(p, tab);
  render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route path="/governance/data/:catalog" element={<Probe />} />
        <Route path="/governance/data/:catalog/:schema" element={<Probe />} />
        <Route path="/governance/data/:catalog/:schema/:object" element={<Probe />} />
      </Routes>
    </MemoryRouter>,
  );
  const out = JSON.parse(screen.getByTestId("path").textContent ?? "null") as ObjectPath;
  return out;
}

describe("objectPath", () => {
  it("builds the URL of each level", () => {
    expect(pathToUrl({ catalog: "ice" })).toBe("/governance/data/ice");
    expect(pathToUrl({ catalog: "ice", schema: "sales" })).toBe("/governance/data/ice/sales");
    expect(pathToUrl({ catalog: "ice", schema: "sales", object: "customers" })).toBe(
      "/governance/data/ice/sales/customers",
    );
    expect(pathToUrl({ catalog: "ice", schema: "sales", object: "customers", column: "email" }, "permissions")).toBe(
      "/governance/data/ice/sales/customers?column=email&tab=permissions",
    );
  });

  it("encodes a space, a % and a / inside a name, so each stays one segment", () => {
    expect(pathToUrl({ catalog: "my cat", schema: "50%", object: "a/b" })).toBe(
      "/governance/data/my%20cat/50%25/a%2Fb",
    );
  });

  it("drops a column on a level that has no object, and an object with no schema", () => {
    expect(pathToUrl({ catalog: "ice", schema: "sales", column: "email" })).toBe("/governance/data/ice/sales");
    expect(pathToUrl({ catalog: "ice", object: "customers" })).toBe("/governance/data/ice");
  });

  it.each<[string, ObjectPath, string | undefined]>([
    ["catalog", { catalog: "ice" }, undefined],
    ["schema", { catalog: "ice", schema: "sales" }, "permissions"],
    ["object", { catalog: "ice", schema: "sales", object: "customers" }, undefined],
    ["column", { catalog: "ice", schema: "sales", object: "customers", column: "e mail" }, "overview"],
    ["names with a space, a % and a /", { catalog: "my cat", schema: "50% off", object: "a/b", column: "x/y%" }, undefined],
    ["a dotted nested schema", { catalog: "ice", schema: "a.b", object: "t" }, undefined],
  ])("round-trips a %s through the router", (_label, p, tab) => {
    expect(roundTrip(p, tab)).toEqual(p);
  });

  it("reads the tab back from the search", () => {
    const search = new URLSearchParams(pathToUrl({ catalog: "ice" }, "details").split("?")[1]);
    expect(search.get("tab")).toBe("details");
  });

  it("parses a dotted path: catalog, catalog.schema, catalog.schema.object, and a nested schema", () => {
    expect(parseDottedPath("ice")).toEqual({ catalog: "ice" });
    expect(parseDottedPath(" ice.sales ")).toEqual({ catalog: "ice", schema: "sales" });
    expect(parseDottedPath("ice.sales.customers")).toEqual({ catalog: "ice", schema: "sales", object: "customers" });
    expect(parseDottedPath("ice.a.b.t")).toEqual({ catalog: "ice", schema: "a.b", object: "t" });
  });

  it("refuses an empty path or an empty part", () => {
    expect(parseDottedPath("")).toBeNull();
    expect(parseDottedPath("ice..t")).toBeNull();
    expect(parseDottedPath("ice.")).toBeNull();
  });
});
