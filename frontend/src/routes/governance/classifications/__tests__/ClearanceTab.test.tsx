import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import type { Tag } from "@/api/tags";
import { fail, ok, openArea, serve, STEWARD, tag, TAG_READER, type Answer } from "./harness";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() } }));

beforeEach(() => vi.clearAllMocks());
afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const URL_TAB = "/governance/classifications/t-pii?tab=clearance";
const POLICY = "/tags/t-pii/access-policy";
const ROLES = [
  { id: "r1", scope: "platform", name: "finance_cleared", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
  { id: "r2", scope: "platform", name: "auditor", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
];
const GROUPS = [
  { id: "g1", name: "Security Team", source: "local", memberCount: 4, roles: [] },
  { id: "g2", name: "Finance Team", source: "local", memberCount: 6, roles: [] },
];

const pii = (over: Partial<Tag> = {}) => tag({ id: "t-pii", key: "pii", name: "PII", ...over });

function open(t: Tag, as = STEWARD, extra: Record<string, Answer> = {}) {
  const served = serve({
    "/tags": ok([t]),
    "/access/roles": ok(ROLES),
    "/groups": ok(GROUPS),
    ...extra,
  });
  openArea(URL_TAB, as);
  return served;
}
const table = () => screen.findByRole("table", { name: "Required roles and groups" }, T);

const GROUP_WARNING = /not enforced in the engine: no group provider or usersync means a group name never reaches trino or starrocks, and a role name does not either/i;
const READS_ONLY = /this clearance withholds reads only, on trino and on starrocks alike: a user without it can still overwrite a column they cannot read/i;
const VIEWS = /trino views in egret.s catalogs carry the classification/i;
const CEILING = /if more than 500 of this tag.s columns are visible to one starrocks profile, that profile.s recompile fails outright rather than truncating the clearance/i;
const COMPILES =
  "Compiles to one deny tag policy in each Trino profile's tag service, and on StarRocks to a select deny and a NULL mask per tagged column. StarRocks does not access-check a masked column, so an uncleared reader reads NULL there rather than being refused.";

async function pickAndAdd(user: ReturnType<typeof userEvent.setup>, type: "Role" | "Group", label: string) {
  if (type === "Group") {
    await user.click(screen.getByRole("combobox", { name: "Clearance type" }));
    await user.click(await screen.findByRole("option", { name: "Group" }));
  }
  await user.click(screen.getByRole("combobox", { name: `${type} to add` }));
  await user.click(await screen.findByRole("button", { name: label }));
  await user.click(screen.getByRole("button", { name: "Add clearance" }));
}

describe("Clearance tab: the editor (ported from TagDetail)", () => {
  it("lists required roles and groups by name, with their type", async () => {
    open(pii({ required_roles: ["finance_cleared"], required_groups: ["g1"] }));
    const t = await table();
    await waitFor(() => expect(within(t).getByText("Security Team")).toBeInTheDocument());
    const rows = within(t).getAllByRole("row").slice(1).map((r) => r.textContent);
    expect(rows).toEqual(["rolefinance_cleared", "groupSecurity Team"]);
  });

  it("adding a role writes the whole set", async () => {
    const user = userEvent.setup();
    const served = open(pii({ required_groups: ["g1"] }), STEWARD, { [`PUT ${POLICY}`]: ok({ ok: true, skipped: [] }) });
    await table();
    await pickAndAdd(user, "Role", "finance_cleared");
    await waitFor(() =>
      expect(served.bodies[`PUT ${POLICY}`]).toEqual([{ required_roles: ["finance_cleared"], required_groups: ["g1"] }]),
    );
    expect(toast.success).toHaveBeenCalledWith("Clearance updated");
  });

  it("adding a group writes the whole set", async () => {
    const user = userEvent.setup();
    const served = open(pii({ required_roles: ["auditor"] }), STEWARD, { [`PUT ${POLICY}`]: ok({ ok: true, skipped: [] }) });
    await table();
    await pickAndAdd(user, "Group", "Finance Team");
    await waitFor(() =>
      expect(served.bodies[`PUT ${POLICY}`]).toEqual([{ required_roles: ["auditor"], required_groups: ["g2"] }]),
    );
  });

  it("removing the last entry clears the policy", async () => {
    const user = userEvent.setup();
    const served = open(pii({ required_roles: ["auditor"] }), STEWARD, { [`DELETE ${POLICY}`]: ok({ ok: true, skipped: [] }) });
    await table();
    await user.click(screen.getByRole("button", { name: "Remove auditor" }));
    await waitFor(() => expect(served.asked).toContain(`DELETE ${POLICY}`));
    expect(toast.success).toHaveBeenCalledWith("Clearance cleared");
  });

  it("removing one of several writes the rest", async () => {
    const user = userEvent.setup();
    const served = open(pii({ required_roles: ["auditor", "finance_cleared"] }), STEWARD, {
      [`PUT ${POLICY}`]: ok({ ok: true, skipped: [] }),
    });
    await table();
    await user.click(screen.getByRole("button", { name: "Remove auditor" }));
    await waitFor(() =>
      expect(served.bodies[`PUT ${POLICY}`]).toEqual([{ required_roles: ["finance_cleared"], required_groups: [] }]),
    );
  });

  it("a write the recompile could not express says so, with a warning toast", async () => {
    const user = userEvent.setup();
    open(pii(), STEWARD, {
      [`PUT ${POLICY}`]: ok({
        ok: true,
        skipped: [
          {
            source_kind: "clearance",
            id: "t-pii",
            reason: "this Trino profile has no usable `tagService`, and a Trino clearance is a TAG policy",
          },
        ],
      }),
    });
    await table();
    await pickAndAdd(user, "Role", "finance_cleared");
    expect(await screen.findByText(/saved, but not compiled into any engine: .*has no usable .?tagService/i, {}, T)).toBeInTheDocument();
    expect(toast.warning).toHaveBeenCalledWith("Clearance saved but not enforced in any engine");
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("a refused write snaps back and shows the server's message", async () => {
    const user = userEvent.setup();
    open(pii(), STEWARD, { [`PUT ${POLICY}`]: fail(400, "unknown role finance_cleared") });
    const t = await table();
    await pickAndAdd(user, "Role", "finance_cleared");
    expect(await screen.findAllByText("unknown role finance_cleared", {}, T)).not.toHaveLength(0);
    await waitFor(() => expect(within(t).queryByText("finance_cleared")).not.toBeInTheDocument());
    expect(within(t).getByText("Not gated — anyone can open objects with this classification.")).toBeInTheDocument();
  });

  it("hides every control without tag:manage or platform admin", async () => {
    open(pii({ required_roles: ["auditor"] }), TAG_READER);
    await table();
    expect(screen.queryByRole("combobox", { name: "Role to add" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add clearance" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove auditor" })).not.toBeInTheDocument();
  });

  it("a reader who cannot list roles or groups asks for neither and sees no load error (live check D1)", async () => {
    const served = open(pii({ required_roles: ["auditor"] }), TAG_READER, {
      "/access/roles": fail(403, "forbidden: role:read"),
      "/groups": fail(403, "forbidden: group:read"),
    });
    const t = await table();
    expect(within(t).getByText("auditor")).toBeInTheDocument();
    expect(screen.queryByText(/could not be loaded/i)).not.toBeInTheDocument();
    expect(served.asked).not.toContain("GET /access/roles");
    expect(served.asked).not.toContain("GET /groups");
  });

  it("a steward refused the role list is told which permission lists roles, not 'No roles found.' (live check D1)", async () => {
    open(pii(), STEWARD, { "/access/roles": fail(403, "forbidden: role:read"), "/groups": fail(403, "forbidden: group:read") });
    await table();
    expect(
      await screen.findByText("Roles are listed with the role:read permission, which this account does not have.", {}, T),
    ).toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Role to add" })).not.toBeInTheDocument();
    expect(screen.queryByText(/could not be loaded/i)).not.toBeInTheDocument();
  });

  it("the picker returns focus to its trigger when a pick closes it, and on Escape", async () => {
    const user = userEvent.setup();
    open(pii());
    await table();
    const trigger = screen.getByRole("combobox", { name: "Role to add" });
    await user.click(trigger);
    await user.click(await screen.findByRole("button", { name: "auditor" }));
    await waitFor(() => expect(trigger).toHaveFocus());
    await user.click(trigger);
    await screen.findByPlaceholderText("Search roles…");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByPlaceholderText("Search roles…")).not.toBeInTheDocument());
    await waitFor(() => expect(trigger).toHaveFocus());
  });
});

describe("Clearance tab: what the engines do not enforce (premise 3)", () => {
  it("an ungated classification shows none of the statements", async () => {
    open(pii());
    await table();
    expect(screen.queryByText(GROUP_WARNING)).not.toBeInTheDocument();
    expect(screen.queryByText(READS_ONLY)).not.toBeInTheDocument();
    expect(screen.queryByText(VIEWS)).not.toBeInTheDocument();
    expect(screen.queryByText(CEILING)).not.toBeInTheDocument();
    expect(screen.queryByText(COMPILES)).not.toBeInTheDocument();
  });

  it("a groups-only gate shows the group warning and none of the role statements", async () => {
    open(pii({ required_groups: ["g1"] }));
    await table();
    expect(screen.getByText(GROUP_WARNING)).toBeInTheDocument();
    expect(screen.queryByText(READS_ONLY)).not.toBeInTheDocument();
    expect(screen.queryByText(/neither engine withholds writes/i)).not.toBeInTheDocument();
    expect(screen.queryByText(VIEWS)).not.toBeInTheDocument();
    expect(screen.queryByText(CEILING)).not.toBeInTheDocument();
    expect(screen.queryByText(COMPILES)).not.toBeInTheDocument();
  });

  it("a role gate states reads only, the views, the ceiling and the compile line, and no group warning", async () => {
    open(pii({ required_roles: ["finance_cleared"] }));
    await table();
    expect(screen.getByText(READS_ONLY)).toBeInTheDocument();
    expect(screen.getByText(/neither engine withholds writes/i)).toBeInTheDocument();
    expect(screen.queryByText(/trino withholds both reads and writes/i)).not.toBeInTheDocument();
    const box = screen.getByText(VIEWS);
    expect(box).toHaveTextContent(/a user without clearance is refused through them/i);
    expect(box).toHaveTextContent(/on starrocks a reader who holds the grant reads null through a plain or nested view/i);
    expect(box).toHaveTextContent(/an invoker view is refused/i);
    expect(box).toHaveTextContent(/existing starrocks materialized views, and views of other connectors, are not covered/i);
    expect(box).toHaveTextContent(/an uncleared owner.s definer view blocks everybody, the cleared included/i);
    expect(screen.getByText(CEILING)).toBeInTheDocument();
    expect(screen.getByText(/profiles that see fewer are unaffected/i)).toBeInTheDocument();
    expect(screen.getByText(COMPILES)).toBeInTheDocument();
    expect(screen.queryByText(GROUP_WARNING)).not.toBeInTheDocument();
  });

  it("a role and a group gate shows all four statements and the compile line", async () => {
    open(pii({ required_roles: ["finance_cleared"], required_groups: ["g1"] }));
    await table();
    expect(screen.getByText(GROUP_WARNING)).toBeInTheDocument();
    expect(screen.getByText(READS_ONLY)).toBeInTheDocument();
    expect(screen.getByText(VIEWS)).toBeInTheDocument();
    expect(screen.getByText(CEILING)).toBeInTheDocument();
    expect(screen.getByText(COMPILES)).toBeInTheDocument();
  });
});
