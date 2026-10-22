import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import type { Tag } from "@/api/tags";
import { ADMIN, fail, ok, openArea, serve, STEWARD, tag, TAG_MANAGER, TAG_READER, type Answer } from "./harness";

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
const table = () => screen.findByRole("table", { name: "Clearance" }, T);

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

  it("Add clearance before a pick stays focusable, says why, and does nothing (N22)", async () => {
    const user = userEvent.setup();
    const served = open(pii({}), STEWARD, { [`PUT ${POLICY}`]: ok({ ok: true, skipped: [] }) });
    await table();
    const add = screen.getByRole("button", { name: "Add clearance" });
    expect(add).not.toBeDisabled();
    expect(add).toHaveAttribute("aria-disabled", "true");
    expect(add).toHaveAccessibleDescription("Pick a role to add.");
    await user.click(add);
    expect(served.bodies[`PUT ${POLICY}`]).toBeUndefined();
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
    expect(within(t).getByText("None: anyone who can read the data can open it.")).toBeInTheDocument();
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
    expect(within(screen.getByRole("tabpanel")).queryByText(/could not be loaded/i)).not.toBeInTheDocument();
    expect(served.asked).not.toContain("GET /access/roles");
    expect(served.asked).not.toContain("GET /groups");
  });

  it("a reader refused the group list sees required groups by id with the permission sentence (D1 style)", async () => {
    const served = open(pii({ required_groups: ["0b6c1f9e-group-id"] }), TAG_READER, {
      "/groups": fail(403, "forbidden: group:read"),
    });
    const t = await table();
    await waitFor(() => expect(served.asked).toContain("GET /groups"));
    expect(within(t).getByText("0b6c1f9e-group-id")).toBeInTheDocument();
    expect(
      await screen.findByText(
        "Groups are listed with the group:read permission, which this account does not have, so required groups are shown by id.",
        {},
        T,
      ),
    ).toBeInTheDocument();
    expect(within(screen.getByRole("tabpanel")).queryByText(/could not be loaded/i)).not.toBeInTheDocument();
  });

  it("a steward refused the role list is told which permission lists roles, not 'No roles found.' (live check D1)", async () => {
    open(pii(), STEWARD, { "/access/roles": fail(403, "forbidden: role:read"), "/groups": fail(403, "forbidden: group:read") });
    await table();
    expect(
      await screen.findByText("Roles are listed with the role:read permission, which this account does not have.", {}, T),
    ).toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Role to add" })).not.toBeInTheDocument();
    expect(within(screen.getByRole("tabpanel")).queryByText(/could not be loaded/i)).not.toBeInTheDocument();
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

describe("Clearance tab: scope first, and who is cleared (C9)", () => {
  const PEOPLE = "/perm/clearances/t-pii/people";
  const NOBODY = { count: 0, people: [], truncated: false, enginesEnforce: false, onEnginesCount: 0 };
  const ENGINES_NOT = "Engines do not enforce this clearance: anyone with SQL access can read the data.";
  const via = (kind: "role" | "group", name: string, extra = {}) => ({
    kind,
    name,
    scope: kind === "role" ? "platform" : null,
    scopeId: null,
    throughGroup: null,
    ...extra,
  });

  it("states what clearance does before anything is added, and who is cleared", async () => {
    open(pii(), ADMIN, { [PEOPLE]: ok(NOBODY), "/governance/catalogs": ok([]) });
    const scope = await screen.findByText(/Clearance decides who may open data with this classification/, undefined, T);
    expect(scope).toBeInTheDocument();
    expect(
      screen.getByText(/A required group is enforced when the data is opened in egret, not in SQL Lab or notebooks/),
    ).toBeInTheDocument();
    expect(within(await table()).getByText("None: anyone who can read the data can open it.")).toBeInTheDocument();
    expect(screen.queryByText(/Not gated/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "add a mask rule" }));
    const dialog = await screen.findByRole("dialog", { name: "New rule" });
    expect(within(dialog).getByLabelText(/^Name/)).toHaveValue("Mask PII");
  });

  it("names the table with the glossary's words, never the retired ones", async () => {
    open(pii({ required_roles: ["auditor"] }), ADMIN, { [PEOPLE]: ok(NOBODY) });
    await table();
    expect(screen.queryByText(/Required roles and groups/)).not.toBeInTheDocument();
  });

  it("lists the people a clearance lists, each once, with qualified copy", async () => {
    open(pii({ required_roles: ["cleared"] }), ADMIN, {
      [PEOPLE]: ok({
        count: 3,
        truncated: false,
        enginesEnforce: true,
        onEnginesCount: 1,
        people: [
          { userId: "u1", label: "Ann", username: "ann", status: "active", serviceAccount: false, via: [via("role", "cleared")], onEngines: true },
          { userId: "u2", label: "Bo", username: null, status: "active", serviceAccount: false, via: [via("group", "data-team")], onEngines: false },
          {
            userId: "u3",
            label: "Cy",
            username: "cy",
            status: "active",
            serviceAccount: false,
            via: [via("role", "cleared", { throughGroup: "ops" })],
            onEngines: false,
          },
        ],
      }),
    });
    await userEvent.click(await screen.findByText("3 people hold a required role or belong to a required group", undefined, T));
    expect(screen.getByText("Platform admins can always open it in egret.")).toBeInTheDocument();
    expect(screen.queryByText(/Engines do not enforce this clearance/)).not.toBeInTheDocument();
    expect(screen.queryByText(/can open it\b/)).not.toBeInTheDocument();
    const list = screen.getByRole("list", { name: "Cleared people" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "Ann · ann · role cleared · engines",
      "Bo · group data-team · Cleared in egret, refused on engines",
      "Cy · cy · role cleared through group ops · Cleared in egret, refused on engines",
    ]);
    // The marker is defined where it appears, and says only what `onEngines`
    // means: excepted from the compiled deny, not able to read.
    expect(within(list).getByTitle("Excepted from the engines' clearance deny (reading still needs a grant).")).toHaveTextContent("engines");
  });

  it("marks no one refused on engines when engines enforce nothing", async () => {
    open(pii({ required_groups: ["g1"] }), ADMIN, {
      [PEOPLE]: ok({
        count: 1,
        truncated: false,
        enginesEnforce: false,
        onEnginesCount: 0,
        people: [{ userId: "u2", label: "Bo", username: "bo", status: "active", serviceAccount: false, via: [via("group", "data-team")], onEngines: false }],
      }),
    });
    await userEvent.click(await screen.findByText("1 person holds a required role or belongs to a required group", undefined, T));
    expect(within(screen.getByRole("list", { name: "Cleared people" })).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "Bo · bo · group data-team",
    ]);
  });

  it("says plainly when the clearance lists no one, with no empty disclosure", async () => {
    open(pii({ required_roles: ["cleared"] }), ADMIN, { [PEOPLE]: ok({ ...NOBODY, enginesEnforce: true }) });
    expect(
      await screen.findByText("No one holds a required role or belongs to a required group yet.", undefined, T),
    ).toBeInTheDocument();
    expect(screen.queryByText(/0 people/)).not.toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Cleared people" })).not.toBeInTheDocument();
  });

  it("names a workspace-scoped role without an id as one workspace", async () => {
    open(pii({ required_roles: ["cleared"] }), ADMIN, {
      [PEOPLE]: ok({
        count: 1,
        truncated: false,
        enginesEnforce: true,
        onEnginesCount: 1,
        people: [
          { userId: "u1", label: "Ann", username: "ann", status: "active", serviceAccount: false, via: [via("role", "cleared", { scope: "workspace", scopeId: null })], onEngines: true },
        ],
      }),
    });
    await userEvent.click(await screen.findByText("1 person holds a required role or belongs to a required group", undefined, T));
    expect(within(screen.getByRole("list", { name: "Cleared people" })).getByRole("listitem")).toHaveTextContent(
      "Ann · ann · role cleared in one workspace only · engines",
    );
  });

  it("states nothing about engines while a clearance change is being saved", async () => {
    const user = userEvent.setup();
    open(pii({ required_groups: ["g1"] }), ADMIN, { [PEOPLE]: ok(NOBODY), [`PUT ${POLICY}`]: "never" });
    expect(await screen.findByText(ENGINES_NOT, undefined, T)).toBeInTheDocument();
    await pickAndAdd(user, "Role", "auditor");
    await waitFor(() => expect(screen.queryByText(ENGINES_NOT)).not.toBeInTheDocument());
    expect(screen.queryByText(/hold a required role|holds a required role|No one holds/)).not.toBeInTheDocument();
  });

  it("offers add a mask rule only with policy:update, and says what it needs otherwise", async () => {
    const served = open(pii(), STEWARD);
    await screen.findByText(/Clearance decides who may open data with this classification/, undefined, T);
    expect(screen.queryByRole("button", { name: "add a mask rule" })).not.toBeInTheDocument();
    expect(screen.getByText("Adding a mask rule needs the policy:update permission.")).toBeInTheDocument();
    expect(served.asked).not.toContain("POST /perm/preview");
  });

  it("qualifies scoped roles, marks disabled and service accounts, and says how many are not listed", async () => {
    open(pii({ required_roles: ["cleared"] }), ADMIN, {
      "/governance/catalogs": ok([{ id: "c1", name: "ice", workspaces: [{ id: "w-1", name: "Sales" }] }]),
      [PEOPLE]: ok({
        count: 203,
        truncated: true,
        enginesEnforce: true,
        onEnginesCount: 2,
        people: [
          {
            userId: "u1",
            label: "Ann",
            username: "ann",
            status: "disabled",
            serviceAccount: false,
            via: [via("role", "cleared", { scope: "workspace", scopeId: "w-1" }), via("group", "ops")],
            onEngines: true,
          },
          {
            userId: "u2",
            label: "Bot",
            username: "bot",
            status: "active",
            serviceAccount: true,
            via: [via("role", "cleared", { scope: "object", scopeId: "o-1" }), via("role", "cleared", { scope: "workspace", scopeId: "w-9" })],
            onEngines: true,
          },
        ],
      }),
    });
    await userEvent.click(await screen.findByText("203 people hold a required role or belong to a required group", undefined, T));
    const list = screen.getByRole("list", { name: "Cleared people" });
    await waitFor(() =>
      expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
        "Ann · ann · role cleared in workspace Sales only, group ops · disabled · engines",
        "Bot · bot · role cleared on one object only, role cleared in workspace w-9 only · service account · engines",
      ]),
    );
    expect(screen.getByText("and 201 more")).toBeInTheDocument();
  });

  it("writes the singular for one person", async () => {
    open(pii({ required_roles: ["cleared"] }), ADMIN, {
      [PEOPLE]: ok({
        count: 1,
        truncated: false,
        enginesEnforce: true,
        onEnginesCount: 1,
        people: [{ userId: "u1", label: "Ann", username: "ann", status: "active", serviceAccount: false, via: [via("role", "cleared")], onEngines: true }],
      }),
    });
    expect(
      await screen.findByText("1 person holds a required role or belongs to a required group", undefined, T),
    ).toBeInTheDocument();
  });

  it("says when engines do not enforce a group-only clearance", async () => {
    open(pii({ required_groups: ["g1"] }), ADMIN, { [PEOPLE]: ok(NOBODY) });
    expect(
      await screen.findByText("Engines do not enforce this clearance: anyone with SQL access can read the data.", undefined, T),
    ).toBeInTheDocument();
  });

  it("without policy:read, says what reading the cleared people needs, and asks nothing", async () => {
    const served = open(pii({ required_roles: ["cleared"] }), TAG_MANAGER);
    expect(await screen.findByText("Seeing who is cleared needs the policy:read permission.", undefined, T)).toBeInTheDocument();
    expect(screen.getByText("Platform admins can always open it in egret.")).toBeInTheDocument();
    expect(served.asked).not.toContain(`GET ${PEOPLE}`);
  });

  it("a failed read of the cleared people is an error with Retry", async () => {
    open(pii({ required_roles: ["cleared"] }), ADMIN, { [PEOPLE]: fail(500) });
    expect(await screen.findByText(/The cleared people could not be loaded/, undefined, T)).toBeInTheDocument();
  });

  it("asks again for the cleared people after a clearance change", async () => {
    const user = userEvent.setup();
    const answers: Record<string, Answer> = {
      "/tags": ok([pii({ required_roles: ["finance_cleared"] })]),
      "/access/roles": ok(ROLES),
      "/groups": ok(GROUPS),
      [PEOPLE]: ok(NOBODY),
      [`PUT ${POLICY}`]: ok({ ok: true, skipped: [] }),
    };
    const served = serve(answers);
    openArea(URL_TAB, ADMIN);
    await table();
    await waitFor(() => expect(served.asked.filter((a) => a === `GET ${PEOPLE}`)).toHaveLength(1));
    // The server now holds the new set.
    answers["/tags"] = ok([pii({ required_roles: ["finance_cleared", "auditor"] })]);
    await pickAndAdd(user, "Role", "auditor");
    await waitFor(() => expect(served.asked.filter((a) => a === `GET ${PEOPLE}`).length).toBeGreaterThan(1));
  });

  it("draws its warnings with tokens, never the raw-palette badge", async () => {
    open(pii({ required_roles: ["cleared"], required_groups: ["g1"] }), ADMIN, { [PEOPLE]: ok(NOBODY) });
    await screen.findByText(/Clearance decides/, undefined, T);
    await table();
    expect(screen.getByText("Engine details")).toBeInTheDocument();
    expect(document.body.querySelector(".bg-yellow-600")).toBeNull();
    const notes = screen.getAllByRole("note");
    // The scope statement is neutral (muted); the measured warnings are highlighted.
    const scope = notes.find((n) => /Clearance decides/.test(n.textContent ?? ""));
    expect(scope?.className).toMatch(/bg-muted/);
    expect(scope?.className).not.toMatch(/highlight/);
    const warnings = notes.filter((n) => n !== scope);
    expect(warnings).toHaveLength(4);
    for (const note of warnings) expect(note.className).toMatch(/bg-highlight\/15/);
  });
});
