import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useParams, useSearchParams } from "react-router-dom";
import type { Ref } from "@/api/generated/model";
import { refUrl } from "@/routes/governance/overview/refUrl";
import { urlToPath, type ObjectPath } from "@/routes/governance/data/objectPath";
import { principalParam } from "@/routes/governance/people/peopleUrl";
import { profileParam } from "@/routes/governance/engines/engineUrl";

/** Read a Data URL back through a REAL router, as `data/__tests__/objectPath.test.tsx` does. */
function DataProbe() {
  const params = useParams();
  const [search] = useSearchParams();
  return <output data-testid="out">{JSON.stringify({ path: urlToPath(params, search), tab: search.get("tab") })}</output>;
}

function NameProbe() {
  const { name } = useParams();
  return <output data-testid="out">{JSON.stringify(principalParam(name))}</output>;
}

function ProfileProbe() {
  const { service } = useParams();
  return <output data-testid="out">{JSON.stringify(profileParam(service))}</output>;
}

function through(url: string): unknown {
  render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route path="/governance/data/:catalog" element={<DataProbe />} />
        <Route path="/governance/data/:catalog/:schema/:object" element={<DataProbe />} />
        <Route path="/governance/people/roles/:name" element={<NameProbe />} />
        <Route path="/governance/engines/profiles/:service" element={<ProfileProbe />} />
      </Routes>
    </MemoryRouter>,
  );
  return JSON.parse(screen.getByTestId("out").textContent ?? "null");
}

describe("refUrl", () => {
  it.each<[Ref, string]>([
    [{ type: "grant", id: "g-1" }, "/governance/grants/g-1"],
    [{ type: "rule", id: "r-1" }, "/governance/rules/r-1"],
    [{ type: "unmatched" }, "/governance/people?kind=unmatched"],
    [{ type: "profile", name: "trino prod" }, "/governance/engines/profiles/trino%20prod"],
    [{ type: "profile", name: "a/b" }, "/governance/engines/profiles/a%2Fb"],
    [{ type: "profiles" }, "/governance/engines"],
    [{ type: "engine", id: "e-1", name: "sr" }, "/control-plane/engines/e-1"],
    [{ type: "engines" }, "/control-plane/engines"],
    [{ type: "catalog", id: "c-1", name: "ice" }, "/control-plane/catalogs/c-1"],
    [{ type: "catalogs" }, "/control-plane/catalogs"],
    [{ type: "object", catalog: "ice", schema: "s", object: "v" }, "/governance/data/ice/s/v?tab=lineage"],
    [{ type: "tag", id: "t-1", name: "PII" }, "/governance/classifications/t-1"],
    [{ type: "tags" }, "/governance/classifications"],
    [{ type: "role", name: "analysts" }, "/governance/people/roles/analysts"],
    [{ type: "data" }, "/governance/data"],
    [{ type: "data_catalog", name: "ice" }, "/governance/data/ice"],
  ])("links %j to %s", (ref, url) => {
    expect(refUrl(ref)).toBe(url);
  });

  it("returns null for a ref it does not know", () => {
    expect(refUrl({ type: "nope" } as unknown as Ref)).toBeNull();
  });

  it.each<[string, ObjectPath]>([
    ["a space, a % and a /", { catalog: "my cat", schema: "50%", object: "a/b" }],
    ["a dot-only name", { catalog: "c", schema: "..", object: "t" }],
  ])("keeps an object named with %s through the router", (_label, p) => {
    const url = refUrl({ type: "object", catalog: p.catalog, schema: p.schema!, object: p.object! })!;
    expect(through(url)).toEqual({ path: p, tab: "lineage" });
  });

  it.each([["a/b"], ["50% off"], ["..."]])("keeps a data catalog named %j through the router", (name) => {
    expect(through(refUrl({ type: "data_catalog", name })!)).toEqual({ path: { catalog: name }, tab: null });
  });

  it.each([["a/b"], ["50% off"], [".."]])("keeps a role named %j through the router", (name) => {
    expect(through(refUrl({ type: "role", name })!)).toBe(name);
  });

  it.each([["a/b"], ["50% off"], [".."]])("keeps a profile named %j through the router", (name) => {
    expect(through(refUrl({ type: "profile", name })!)).toBe(name);
  });

  // #4a spec §3: the Overview opens a profile on the tab where its finding is fixed.
  it("puts the tab on a profile link", () => {
    expect(refUrl({ type: "profile", name: "a/b" }, "plugin")).toBe("/governance/engines/profiles/a%2Fb?tab=plugin");
    expect(refUrl({ type: "profile", name: "p" }, "overlaps")).toBe("/governance/engines/profiles/p?tab=overlaps");
  });

  it("ignores the tab on a ref that is not a profile", () => {
    expect(refUrl({ type: "profiles" }, "plugin")).toBe("/governance/engines");
    expect(refUrl({ type: "engine", id: "e-1", name: "sr" }, "plugin")).toBe("/control-plane/engines/e-1");
    expect(refUrl({ type: "rule", id: "r-1" }, "overlaps")).toBe("/governance/rules/r-1");
  });
});
