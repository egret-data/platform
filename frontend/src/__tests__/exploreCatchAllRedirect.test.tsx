import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Navigate, Route, Routes, useLocation, useParams } from "react-router-dom";
import { exploreRedirectTo } from "@/hooks/useWorkspaceFeatures";

/**
 * The catch-all under `/explore/:workspace/*` sends unknown URLs to the workspace's
 * default module. Reproduced here rather than importing `App`, because `App`'s tree
 * needs auth/workspace/feature providers and none of them are what this is about: the
 * bug is purely how a RELATIVE `<Navigate>` resolves from inside a splat route.
 *
 * react-router 6 resolved it against the splat route's own pattern
 * (`/explore/:workspace/`), so `"sql-lab"` meant `/explore/default/sql-lab`.
 * react-router 7 resolves it against the MATCHED location instead, so on
 * `/explore/default/databases` the same `"sql-lab"` means
 * `/explore/default/databases/sql-lab` — which matches `*` again, redirects again, and
 * the path grows a `/sql-lab` per pass forever. Found by opening an unknown explore URL
 * in a browser; the suite was green and had nothing to say about it.
 *
 * The redirect is BUDGETED below rather than left to run. Left alone this loop does not
 * trip React's update-depth guard — each hop is its own effect tick — so an unbudgeted
 * version of this test hangs the runner instead of failing it.
 */
const HOP_BUDGET = 5;

function Probe() {
  return <div data-testid="path">{useLocation().pathname}</div>;
}

/**
 * A `<Navigate>` that gives up after `HOP_BUDGET` hops, so a loop reports instead of
 * spinning. The target comes from `exploreRedirectTo` — the same helper `App` uses — so
 * this asserts our redirect, not react-router's resolution rules in the abstract.
 */
function BudgetedNavigate({ defaultPath, hops }: { defaultPath: string; hops: { n: number } }) {
  const path = useLocation().pathname;
  const { workspace } = useParams();
  if (hops.n >= HOP_BUDGET) return <div data-testid="path">LOOPED after {hops.n} hops: {path}</div>;
  hops.n += 1;
  return <Navigate to={exploreRedirectTo(workspace, defaultPath)} replace />;
}

function renderAt(initial: string, defaultPath = "sql-lab") {
  const hops = { n: 0 };
  render(
    <MemoryRouter initialEntries={[initial]}>
      <Routes>
        <Route
          path="/explore/:workspace/*"
          element={
            <Routes>
              <Route index element={<BudgetedNavigate defaultPath={defaultPath} hops={hops} />} />
              <Route path="sql-lab" element={<Probe />} />
              <Route path="settings/members" element={<Probe />} />
              <Route path="*" element={<BudgetedNavigate defaultPath={defaultPath} hops={hops} />} />
            </Routes>
          }
        />
      </Routes>
    </MemoryRouter>,
  );
  return hops;
}

describe("explore catch-all redirect", () => {
  it("sends an unknown explore URL to the workspace's default module in ONE hop", () => {
    const hops = renderAt("/explore/default/databases");
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/default/sql-lab");
    expect(hops.n, "more than one hop means the target is being re-matched").toBe(1);
  });

  it("does not append the default module onto the unknown path", () => {
    renderAt("/explore/default/databases");
    expect(screen.getByTestId("path").textContent).not.toMatch(/databases/);
  });

  it("still redirects the bare workspace index", () => {
    renderAt("/explore/default");
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/default/sql-lab");
  });

  it("handles a multi-segment default path", () => {
    // A default path of two segments, e.g. one Workspace settings section.
    renderAt("/explore/default/nope", "settings/members");
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/default/settings/members");
  });
});
