import { render, screen } from "@testing-library/react";
import {
  createMemoryRouter,
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  useLocation,
} from "react-router-dom";
import { describe, expect, it } from "vitest";
import { LegacyGovernanceRedirect, RetiredPermPageRedirect } from "@/routes/governance/LegacyGovernanceRedirect";

function Where({ app }: { app: string }) {
  const { pathname, search, hash } = useLocation();
  return <div data-testid="where">{`${app} ${pathname}${search}${hash}`}</div>;
}

/**
 * The same route shapes App mounts. `/control-plane/*` is declared FIRST on
 * purpose: App relies on react-router 7 ranking the legacy routes above that
 * splat by specificity, not by declaration order.
 */
function renderAt(url: string) {
  render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route path="/control-plane/*" element={<Where app="control-plane" />} />
        <Route path="/control-plane/policies/*" element={<LegacyGovernanceRedirect />} />
        <Route path="/control-plane/classifications/*" element={<LegacyGovernanceRedirect />} />
        <Route path="/control-plane/data-lineage" element={<LegacyGovernanceRedirect />} />
        <Route path="/governance/*" element={<Where app="governance" />} />
      </Routes>
    </MemoryRouter>,
  );
  return screen.getByTestId("where").textContent;
}

describe("LegacyGovernanceRedirect", () => {
  it.each([
    ["/control-plane/policies", "governance /governance/policies"],
    ["/control-plane/policies/", "governance /governance/policies/"],
    ["/control-plane/policies/trino-prod", "governance /governance/policies/trino-prod"],
    [
      "/control-plane/policies/trino-prod/access/new?scope=tag#deny",
      "governance /governance/policies/trino-prod/access/new?scope=tag#deny",
    ],
    ["/control-plane/policies/trino-prod/masking/42", "governance /governance/policies/trino-prod/masking/42"],
    ["/control-plane/classifications", "governance /governance/classifications"],
    ["/control-plane/classifications/t1?tab=clearance", "governance /governance/classifications/t1?tab=clearance"],
    ["/control-plane/data-lineage", "governance /governance/data-lineage"],
  ])("sends %s to Governance with subpath, search and hash", (from, to) => {
    expect(renderAt(from)).toBe(to);
  });

  it.each(["/control-plane/audit-log", "/control-plane/engines", "/control-plane"])(
    "leaves %s in Control Plane",
    (url) => {
      expect(renderAt(url)).toBe(`control-plane ${url}`);
    },
  );

  it("replaces the history entry so Back does not bounce through the old URL", async () => {
    const router = createMemoryRouter(
      [
        { path: "/control-plane/policies/*", element: <LegacyGovernanceRedirect /> },
        { path: "/governance/*", element: <Where app="governance" /> },
      ],
      { initialEntries: ["/control-plane/policies/trino-prod?scope=tag"] },
    );
    render(<RouterProvider router={router} />);
    expect(await screen.findByTestId("where")).toHaveTextContent("governance /governance/policies/trino-prod?scope=tag");
    expect(router.state.historyAction).toBe("REPLACE");
  });
});

/**
 * DG redesign #2, spec P8 and §5: Explain access, Data permissions and Mask &
 * filter rules are gone. Their URLs open People & groups; a deep link to one
 * grant or rule (`?grant=`, `?rule=`, which explain and the overlap report
 * used to write) opens that grant's or rule's own page.
 */
describe("RetiredPermPageRedirect", () => {
  function renderRetired(url: string) {
    const router = createMemoryRouter(
      [
        { path: "/governance/data-permissions", element: <RetiredPermPageRedirect /> },
        { path: "/governance/mask-filter-rules", element: <RetiredPermPageRedirect /> },
        { path: "/governance/explain-access", element: <RetiredPermPageRedirect /> },
        { path: "/governance/*", element: <Where app="governance" /> },
      ],
      { initialEntries: [url] },
    );
    render(<RouterProvider router={router} />);
    return router;
  }

  it.each([
    ["/governance/explain-access", "governance /governance/people"],
    ["/governance/data-permissions", "governance /governance/people"],
    ["/governance/mask-filter-rules", "governance /governance/people"],
    ["/governance/data-permissions?grant=g-1", "governance /governance/grants/g-1"],
    ["/governance/mask-filter-rules?rule=r-1", "governance /governance/rules/r-1"],
    ["/governance/mask-filter-rules?rule=", "governance /governance/people"],
    ["/governance/data-permissions?grant=a%2Fb", "governance /governance/grants/a%2Fb"],
  ])("sends %s to %s, with replace", async (from, to) => {
    const router = renderRetired(from);
    expect((await screen.findByTestId("where")).textContent).toBe(to);
    expect(router.state.historyAction).toBe("REPLACE");
  });
});
