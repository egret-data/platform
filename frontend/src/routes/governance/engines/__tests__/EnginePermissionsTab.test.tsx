import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import {
  CAPABILITIES,
  E_OTHER,
  E_TR,
  engineGrant,
  fail,
  grantsResponse,
  managedEntry,
  ok,
  openArea,
  policy,
  profileDto,
  READER,
  serve,
  STEWARD,
  type Answer,
} from "./engineHarness";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

beforeEach(() => {
  vi.mocked(toast.success).mockClear();
  vi.mocked(toast.error).mockClear();
  vi.spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(() => undefined);
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const T = { timeout: 3000 };

// Not in the catalogue's order, nor grouped: the tab must sort and group (FE-TEST-1).
const GRANTS = [
  engineGrant({
    id: "g-sess",
    capability: "trino.set_catalog_session_property",
    values: ["ice", "query_max_memory"],
    principals: [{ kind: "group", ref: "analysts" }],
  }),
  engineGrant({
    id: "g-imp-ub",
    capability: "trino.impersonate_user",
    values: ["ub"],
    targetUserId: "u-ub",
    principals: [{ kind: "user", ref: "ua" }],
  }),
  engineGrant({
    id: "g-imp-gone",
    capability: "trino.impersonate_user",
    values: [],
    targetUserId: "u-nameless",
    principals: [{ kind: "role", ref: "etl" }],
    engineId: E_OTHER,
    engineName: "zeta",
  }),
];

const MANAGED = [
  managedEntry({
    policyId: 11,
    kind: "delegator",
    capability: "trino.impersonate_user",
    resources: { trinouser: ["*"] },
    accesses: ["impersonate"],
    users: ["egret-delegator"],
  }),
  managedEntry({
    policyId: 12,
    kind: "admin_all_access",
    resources: { catalog: ["*"] },
    accesses: ["all"],
    groups: ["platform_admins"],
    enabled: false,
  }),
];

function answers(over: Record<string, Answer> = {}): Record<string, Answer> {
  return {
    [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse({ grants: GRANTS, managed: MANAGED })),
    "/perm/engine-capabilities": ok(CAPABILITIES),
    "/policies/services": ok([profileDto("lake-tr", { engines: [{ id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" }] })]),
    "/policies?service=lake-tr": ok([policy(1, "compiler")]),
    ...over,
  };
}

const section = (name: string) => screen.findByRole("region", { name }, T);

describe("Engine permissions: granted (spec §6.1)", () => {
  it("groups grants by capability in the catalogue's order, with label and description", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, READER);
    const granted = await section("Granted");
    const groups = await within(granted).findAllByRole("heading", { level: 3 }, T);
    expect(groups.map((h) => h.textContent)).toEqual(["Impersonate a user", "Set catalog session properties"]);
    expect(within(granted).getByText("Run queries as another user.")).toBeInTheDocument();
    const imp = within(granted).getByRole("region", { name: "Impersonate a user" });
    expect(within(imp).getAllByRole("row")).toHaveLength(3); // header + 2
  });

  it("links an impersonation target to the People user page, and names a target with no username", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, READER);
    const granted = await section("Granted");
    expect(await within(granted).findByRole("link", { name: "ub" }, T)).toHaveAttribute(
      "href",
      "/governance/people/users/u-ub",
    );
    expect(within(granted).getByText("a user with no username")).toBeInTheDocument();
  });

  it("shows plain values, linked principals and the generated policy name", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, READER);
    const sess = await screen.findByRole("region", { name: "Set catalog session properties" }, T);
    expect(within(sess).getByText("ice")).toBeInTheDocument();
    expect(within(sess).getByText("query_max_memory")).toBeInTheDocument();
    expect(within(sess).getByRole("link", { name: "group analysts" })).toHaveAttribute(
      "href",
      "/governance/people/groups/analysts",
    );
    expect(within(sess).getByText("perm-engine-g-sess")).toBeInTheDocument();
    const imp = screen.getByRole("region", { name: "Impersonate a user" });
    expect(within(imp).getByRole("link", { name: "user ua" })).toHaveAttribute(
      "href",
      "/governance/people/users/by-name/ua",
    );
    expect(within(imp).getByRole("link", { name: "role etl" })).toHaveAttribute("href", "/governance/people/roles/etl");
  });

  it("says written on engine X only for a grant written on another engine", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, READER);
    const granted = await section("Granted");
    expect(await within(granted).findByText("Written on engine zeta", {}, T)).toBeInTheDocument();
    expect(within(granted).getAllByText(/Written on engine/)).toHaveLength(1);
  });

  it("highlights and scrolls to the row ?grant= names", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}?grant=g-sess`, READER);
    const sess = await screen.findByRole("region", { name: "Set catalog session properties" }, T);
    const row = within(sess).getByText("perm-engine-g-sess").closest("tr");
    expect(row).toHaveAttribute("aria-current", "true");
    await waitFor(() => expect(HTMLElement.prototype.scrollIntoView).toHaveBeenCalled());
    const other = screen.getByText("perm-engine-g-imp-ub").closest("tr");
    expect(other).not.toHaveAttribute("aria-current");
  });

  it("says nothing has been granted when the list is empty", async () => {
    serve(answers({ [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse({ managed: MANAGED })) }));
    openArea(`/governance/engines/${E_TR}`, READER);
    expect(
      await screen.findByText("No engine permission has been granted on this engine.", {}, T),
    ).toBeInTheDocument();
  });

  it("says nothing can be granted on an engine with no profile", async () => {
    serve(answers({ [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse({ profile: null })) }));
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    expect(
      await screen.findByText(
        "This engine downloads no policy profile, so nothing can be granted on it. Bind a profile in Control plane.",
        {},
        T,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Granted" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Grant/ })).not.toBeInTheDocument();
  });
});

describe("Engine permissions: writes", () => {
  it("offers no Edit or Delete without policy:update", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, READER);
    await section("Granted");
    await screen.findByText("perm-engine-g-sess", {}, T);
    expect(screen.queryByRole("button", { name: /^Delete/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Edit/ })).not.toBeInTheDocument();
  });

  it("deletes after confirming, then refetches and toasts", async () => {
    const { asked } = serve(
      answers({ "DELETE /perm/engine-grants/g-sess": { status: 204, body: null } }),
    );
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    const del = await screen.findByRole(
      "button",
      { name: "Delete engine permission perm-engine-g-sess" },
      T,
    );
    const before = asked.filter((a) => a === `GET /perm/engines/${E_TR}/grants`).length;
    await userEvent.click(del);
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.type(within(dialog).getByRole("textbox"), "delete");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(asked).toContain("DELETE /perm/engine-grants/g-sess"));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Engine permission deleted."));
    await waitFor(() =>
      expect(asked.filter((a) => a === `GET /perm/engines/${E_TR}/grants`).length).toBeGreaterThan(before),
    );
  });

  it("shows the server's refusal", async () => {
    serve(answers({ "DELETE /perm/engine-grants/g-sess": fail(409, "the profile failed to compile: overlap") }));
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    await userEvent.click(
      await screen.findByRole("button", { name: "Delete engine permission perm-engine-g-sess" }, T),
    );
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.type(within(dialog).getByRole("textbox"), "delete");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("the profile failed to compile: overlap"));
  });
});

describe("Engine permissions: managed by egret and not grantable", () => {
  it("lists each managed entry with its kind label and mapped capability, read-only", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, STEWARD);
    await section("Not grantable here"); // the catalogue has answered: capability labels are known
    const managed = await section("Managed by egret");
    const rows = within(managed).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByText("egret's delegator")).toBeInTheDocument();
    expect(within(rows[0]).getByText("Impersonate a user")).toBeInTheDocument();
    expect(within(rows[0]).getByText("trinouser=*")).toBeInTheDocument();
    expect(within(rows[0]).getByText("impersonate")).toBeInTheDocument();
    expect(within(rows[0]).getByText("user egret-delegator")).toBeInTheDocument();
    expect(within(rows[0]).getByText("Enabled")).toBeInTheDocument();
    expect(within(rows[1]).getByText("Platform admins: all access")).toBeInTheDocument();
    expect(within(rows[1]).getByText("group platform_admins")).toBeInTheDocument();
    expect(within(rows[1]).getByText("Disabled")).toBeInTheDocument();
    expect(within(managed).queryByRole("button")).not.toBeInTheDocument();
  });

  it("lists what the engine type cannot grant, with the way to Advanced", async () => {
    serve(answers());
    openArea(`/governance/engines/${E_TR}`, READER);
    const ng = await section("Not grantable here");
    expect(within(ng).getByText("Query another user's queries")).toBeInTheDocument();
    expect(within(ng).getByText("Ranger's Trino plugin has no such resource.")).toBeInTheDocument();
    expect(within(ng).queryByText("Grant a role")).not.toBeInTheDocument();
    expect(within(ng).getByRole("link", { name: "Advanced" })).toHaveAttribute(
      "href",
      `/governance/engines/${E_TR}?tab=advanced`,
    );
  });

  it("shows a failing capability catalogue with Retry and still lists the grants", async () => {
    serve(answers({ "/perm/engine-capabilities": fail(500) }));
    openArea(`/governance/engines/${E_TR}`, READER);
    expect(await screen.findByRole("button", { name: "Retry" }, T)).toBeInTheDocument();
    expect(screen.getByText("perm-engine-g-sess")).toBeInTheDocument();
  });
});
