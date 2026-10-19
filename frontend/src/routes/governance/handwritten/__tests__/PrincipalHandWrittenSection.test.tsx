import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { PrincipalHandWritten, PrincipalPolicy } from "@/api/generated/model";
import type { Me } from "@/api/hooks";
import { AuthContext } from "@/auth/useAuth";
import { fail, me, ok, serve, type Answer } from "@/routes/governance/engines/__tests__/engineHarness";
import {
  PrincipalHandWrittenSection,
  type PrincipalSubject,
} from "@/routes/governance/handwritten/PrincipalHandWrittenSection";

afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const READER = me(["policy:read"]);
const USER: PrincipalSubject = { kind: "user", id: "u-ana" };
const KEY = "GET /governance/principals/user/u-ana/handwritten";

function policy(over: Partial<PrincipalPolicy> & Pick<PrincipalPolicy, "id" | "name">): PrincipalPolicy {
  return {
    service: "lake-tr",
    inTagService: false,
    policyType: "access",
    resourceSummary: "catalog=ice; schema=sales",
    enabled: true,
    denyAllElse: false,
    unmodelled: [],
    naming: [{ bucket: "allow", how: "direct", via: null, onEngines: true }],
    engines: [{ id: "e-tr", name: "lake-trino" }],
    ...over,
  };
}

function answer(over: Partial<PrincipalHandWritten> = {}): PrincipalHandWritten {
  return {
    principal: { kind: "user", id: "u-ana", label: "Ana", username: "ana" },
    state: "listed",
    policies: [],
    everyone: { count: 0, policies: [] },
    ...over,
  };
}

function open(a: Answer, subject: PrincipalSubject = USER, show: "access" | "rules" = "access", as: Me = READER, key = KEY) {
  const served = serve({ [key]: a });
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me: as, isLoading: false }}>
        <MemoryRouter>
          <PrincipalHandWrittenSection subject={subject} show={show} />
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  return served;
}

const group = (name: string) => screen.getByRole("region", { name });

describe("PrincipalHandWrittenSection (route B)", () => {
  it("groups a user's rows: Direct, Through a group or role (Q1), Applies to everyone", async () => {
    const user = userEvent.setup();
    open(
      ok(
        answer({
          policies: [
            policy({ id: 2, name: "via-g1", naming: [{ bucket: "allow", how: "group", via: "g1", onEngines: false }] }),
            policy({ id: 1, name: "direct-deny", naming: [{ bucket: "deny", how: "direct", via: null, onEngines: true }] }),
            policy({ id: 3, name: "via-r1", naming: [{ bucket: "allow", how: "role", via: "r1", onEngines: false }] }),
          ],
          everyone: {
            count: 2,
            policies: [
              { id: 9, name: "all-read", service: "lake-tr", bucket: "allow" },
              { id: 8, name: "all-mask", service: "lake-tr", bucket: "mask" },
            ],
          },
        }),
      ),
    );
    expect(await screen.findByRole("region", { name: "Hand-written policies (3)" }, T)).toBeInTheDocument();
    expect(within(group("Direct")).getByRole("link", { name: "direct-deny" })).toHaveAttribute(
      "href",
      "/governance/engines/e-tr/advanced/policies/access/1",
    );
    expect(within(group("Direct")).getByText("Deny")).toBeInTheDocument();
    const via = group("Through a group or role");
    expect(within(via).getAllByRole("link").map((l) => l.textContent)).toEqual(["via-g1", "via-r1"]);
    expect(via).toHaveTextContent(
      "Names group g1, which this user is in. Engines resolve no groups, so on the engine it reaches nobody.",
    );
    expect(via).toHaveTextContent(
      "Names role r1, which this user holds. Engines resolve no roles, so on the engine it reaches nobody.",
    );
    const everyone = group("Applies to everyone");
    expect(everyone).toHaveTextContent("1 hand-written policy names everyone.");
    expect(within(everyone).queryByText("all-read")).not.toBeInTheDocument();
    await user.click(within(everyone).getByRole("button", { name: "Show them" }));
    expect(within(everyone).getByText("all-read")).toBeInTheDocument();
    expect(within(everyone).queryByText("all-mask")).not.toBeInTheDocument();
  });

  it("on a group's page, its own name is marked as reaching nobody", async () => {
    open(
      ok(
        answer({
          principal: { kind: "group", id: "g1", label: "g1", username: null },
          policies: [policy({ id: 2, name: "names-g1", naming: [{ bucket: "allow", how: "group", via: "g1", onEngines: false }] })],
        }),
      ),
      { kind: "group", id: "g1" },
      "access",
      READER,
      "GET /governance/principals/group/g1/handwritten",
    );
    await screen.findByRole("link", { name: "names-g1" }, T);
    expect(screen.getByRole("region", { name: "Through a group or role" })).toHaveTextContent(
      "Names group g1. Engines resolve no groups, so on the engine it reaches nobody.",
    );
  });

  it("states 'reaches nobody' only where the server says the engines do not apply the item", async () => {
    open(ok(answer({ policies: [policy({ id: 2, name: "p", naming: [{ bucket: "allow", how: "group", via: "g9", onEngines: true }] })] })));
    await screen.findByRole("link", { name: "p" }, T);
    expect(screen.getByRole("region", { name: "Through a group or role" })).not.toHaveTextContent(/reaches nobody/);
  });

  it("a role's page asks route B by the role's name", async () => {
    const served = open(ok(answer({ principal: { kind: "role", id: "r1", label: "r1", username: null } })), { kind: "role", id: "r1" }, "access", READER, "GET /governance/principals/role/r1/handwritten");
    await screen.findByText("No hand-written policy names this principal.", {}, T);
    expect(served.asked).toEqual(["GET /governance/principals/role/r1/handwritten"]);
  });

  it("shows masking and row-filter rows on the rules placement, access rows on the access one", async () => {
    const body = answer({
      policies: [policy({ id: 1, name: "acc" }), policy({ id: 2, name: "msk", policyType: "masking", naming: [{ bucket: "mask", how: "direct", via: null, onEngines: true }] })],
    });
    open(ok(body), USER, "rules");
    await screen.findByRole("link", { name: "msk" }, T);
    expect(screen.queryByRole("link", { name: "acc" })).not.toBeInTheDocument();
  });

  it("a user without a username: no item can name them; only everyone is counted", async () => {
    open(ok(answer({ state: "no_username", principal: { kind: "user", id: "u-ana", label: "Ana", username: null }, everyone: { count: 1, policies: [{ id: 9, name: "all-read", service: "lake-tr", bucket: "allow" }] } })));
    expect(
      await screen.findByText("This user has no username, so no hand-written policy item can name them on an engine.", {}, T),
    ).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Applies to everyone" })).toHaveTextContent("1 hand-written policy names everyone.");
    expect(screen.queryByRole("region", { name: "Direct" })).not.toBeInTheDocument();
  });

  it("explains, and asks nothing, without policy:read", async () => {
    const served = open(ok(answer()), USER, "access", me(["tag:read"]));
    expect(
      await screen.findByText(
        "Hand-written policies are read with the policy:read permission, which this account does not have.",
        {},
        T,
      ),
    ).toBeInTheDocument();
    expect(served.asked).toEqual([]);
  });

  it("shows a failure with Retry, never none", async () => {
    const user = userEvent.setup();
    const served = open(fail(500));
    expect(await screen.findByText(/The hand-written policies could not be loaded/, {}, T)).toBeInTheDocument();
    expect(screen.queryByText("No hand-written policy names this principal.")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(served.asked.filter((a) => a === KEY)).toHaveLength(2));
  });
});
