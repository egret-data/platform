import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import { AuthContext } from "@/auth/useAuth";
import { fail, me, ok, serve } from "@/routes/governance/engines/__tests__/engineHarness";
import { NotEnforcedBadge, useRuleEnforcement } from "@/routes/governance/NotEnforced";
import { notEnforcedReason } from "@/routes/governance/ruleEffect";

afterEach(() => vi.unstubAllGlobals());

describe("notEnforcedReason", () => {
  it("names each reason in the preview's words, by what the rule is written on", () => {
    expect(notEnforcedReason("no_engine", "tag")).toBe("no engine attached");
    expect(notEnforcedReason("no_profile", "path")).toBe("its engines have no policy profile");
    expect(notEnforcedReason("no_assignment", "tag")).toBe("this classification is not attached to any data a catalog serves");
    expect(notEnforcedReason("no_assignment", "path")).toBe("no catalog egret knows serves this path");
    expect(notEnforcedReason(null, "tag")).toBe("no engine enforces it");
  });
});

describe("NotEnforcedBadge", () => {
  it("says the rule is not enforced and why, in words", () => {
    const { container } = render(<NotEnforcedBadge enforcement={{ enforcedBy: 0, notEnforced: "no_engine" }} appliesTo="path" />);
    expect(container).toHaveTextContent("Not enforced yet — no engine attached");
  });

  it.each([
    ["an enforced rule", { enforcedBy: 1, notEnforced: null }],
    ["an unknown answer", null],
    ["no answer for this rule", undefined],
  ])("renders nothing for %s", (_, enforcement) => {
    const { container } = render(<NotEnforcedBadge enforcement={enforcement} appliesTo="tag" />);
    expect(container).toBeEmptyDOMElement();
  });
});

function Probe({ ids }: { ids: string[] }) {
  const map = useRuleEnforcement(ids);
  if (map === null) return <p>unknown</p>;
  return (
    <ul>
      {ids.map((id) => (
        <li key={id}>
          {id}: <NotEnforcedBadge enforcement={map.get(id)} appliesTo="tag" />
        </li>
      ))}
    </ul>
  );
}

function renderProbe(ids: string[], as: Me) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me: as, isLoading: false }}>
        <Probe ids={ids} />
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

describe("useRuleEnforcement", () => {
  it("asks once for the distinct ids, sorted, and answers per rule", async () => {
    const { asked } = serve({
      "/perm/rules/enforcement?ids=a,b": ok([
        { ruleId: "a", enforcedBy: 0, notEnforced: "no_profile" },
        { ruleId: "b", enforcedBy: 2, notEnforced: null },
      ]),
    });
    renderProbe(["b", "a", "b"], me(["policy:read"]));
    await waitFor(() => expect(screen.getAllByRole("listitem")[1]).toHaveTextContent("a: Not enforced yet — its engines have no policy profile"));
    expect(screen.getAllByRole("listitem")[0]).toHaveTextContent(/^b:\s*$/);
    expect(asked.filter((k) => k.includes("/perm/rules/enforcement"))).toEqual(["GET /perm/rules/enforcement?ids=a,b"]);
  });

  it("claims nothing while it cannot know: no permission, a failed answer", async () => {
    const { asked } = serve({ "/perm/rules/enforcement?ids=a": fail(500, "boom") });
    renderProbe(["a"], me([]));
    expect(screen.getByText("unknown")).toBeInTheDocument();
    expect(asked).toEqual([]);

    renderProbe(["a"], me(["policy:read"]));
    await waitFor(() => expect(asked).toContain("GET /perm/rules/enforcement?ids=a"));
    expect(screen.getAllByText("unknown")).toHaveLength(2);
    expect(screen.queryByText(/Not enforced yet/)).not.toBeInTheDocument();
  });

  it("asks for at most 100 rules at once", async () => {
    const ids = Array.from({ length: 101 }, (_, i) => `r${String(i).padStart(3, "0")}`);
    const { asked } = serve({});
    renderProbe(ids, me(["policy:read"]));
    await waitFor(() => expect(asked.length).toBeGreaterThan(0));
    const sent = asked[0].split("ids=")[1].split(",");
    expect(sent).toHaveLength(100);
    expect(sent).not.toContain("r100");
  });
});

describe("invalidateRuleEnforcement", () => {
  it("marks every enforcement and effect answer stale, and nothing else", async () => {
    const { invalidateRuleEnforcement } = await import("@/routes/governance/NotEnforced");
    const { invalidateTagAssignment } = await import("@/api/tags");
    const qc = new QueryClient();
    const keys = [
      ["/api/control-plane/perm/rules/enforcement", { ids: "a" }],
      ["/api/control-plane/perm/rules/r1/effect"],
      ["/api/control-plane/perm/rules"],
    ];
    for (const k of keys) qc.setQueryData(k, { status: 200, data: [] });
    invalidateRuleEnforcement(qc);
    const stale = keys.map((k) => qc.getQueryState(k)?.isInvalidated);
    expect(stale).toEqual([true, true, false]);

    // Attaching or removing a classification changes what a classification rule reaches.
    const qc2 = new QueryClient();
    qc2.setQueryData(keys[0], { status: 200, data: [] });
    invalidateTagAssignment(qc2, "t1", "wh1");
    expect(qc2.getQueryState(keys[0])?.isInvalidated).toBe(true);
  });
});
