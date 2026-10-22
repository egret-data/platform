import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ADMIN, fail, me, ok, openArea, serve, STEWARD, tag, TAG_READER, where, type Answer } from "./harness";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() } }));
afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const PII = tag({ id: "t-pii", key: "pii", name: "PII" });
const USAGE_NONE = {
  tag: { id: "t-pii", key: "pii", name: "PII" },
  data: { tables: [], tableCount: 0, columnCount: 0, orphaned: 0 },
  bi: { readable: [], unreadable: { count: 0 }, deleted: 0, checked: 0, remaining: 0 },
};
const USAGE_TWO = { ...USAGE_NONE, data: { ...USAGE_NONE.data, tableCount: 2, columnCount: 3 } };
const HASH = {
  id: "r1",
  name: "Hash email",
  kind: { type: "mask", mask_type: "MASK_HASH" },
  selector: { type: "tag", key: "pii" },
  exempt: { include: [], exclude: [] },
  tag_name: "PII",
};
const NULLIFY = { ...HASH, id: "r2", name: "Null phone", kind: { type: "mask", mask_type: "MASK_NULL" } };
const strip = () => screen.getByRole("region", { name: "Effect" });
const PREVIEWED = { affected: [], compile: { services: [], created: 0, updated: 0, deleted: 0, unchanged: 0, skipped: [] } };
/** May attach classifications in Data (`catalog_registry:update`), nothing else beyond reading. */
const DATA_EDITOR = me(["tag:read", "policy:read", "catalog_registry:update"]);

describe("the Effect strip", () => {
  it("says a new classification does nothing yet, and how to change that", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_NONE), "/perm/rules": ok([]) });
    openArea("/governance/classifications/t-pii", ADMIN);
    await screen.findByRole("heading", { level: 1, name: "PII" }, T);
    await waitFor(() => expect(strip()).toHaveTextContent("Does nothing yet."));
    expect(strip()).toHaveTextContent("Nothing uses this classification yet");
    expect(within(strip()).getByText("No rule uses this classification yet.")).toBeInTheDocument();
    expect(within(strip()).getByText("None: anyone who can read the data can open it.")).toBeInTheDocument();
    expect(within(strip()).getByText("Not on any data yet.")).toBeInTheDocument();
    expect(within(strip()).getByRole("link", { name: "Attach in Data" })).toHaveAttribute("href", "/governance/data");
    expect(strip()).toHaveTextContent("Open a table or a column and use Add classification.");
    expect(within(strip()).getByRole("button", { name: "Add rule" })).toBeInTheDocument();
    await userEvent.click(within(strip()).getByRole("button", { name: "Set clearance" }));
    expect(where()).toBe("/governance/classifications/t-pii?tab=clearance");
  });

  it("states rules, clearance and usage when they exist", async () => {
    serve({
      "/tags": ok([{ ...PII, required_groups: ["g1", "g2"] }]),
      "/classifications/t-pii/usage": ok(USAGE_TWO),
      "/perm/rules": ok([NULLIFY, HASH]),
    });
    openArea("/governance/classifications/t-pii", ADMIN);
    await waitFor(() => expect(strip()).toHaveTextContent("Mask · Hash: Hash email and 1 more"), T);
    expect(
      within(strip()).getByText(
        "Only members of 2 groups, and platform admins, may open data with it in egret. Engines do not enforce it.",
      ),
    ).toBeInTheDocument();
    expect(strip()).toHaveTextContent("2 tables · 3 columns · 0 BI objects");
    expect(strip()).not.toHaveTextContent("Does nothing yet.");
  });

  it("Attach in Data remembers the classification for the Add classification dialog there", async () => {
    const { attachIntent } = await import("@/routes/governance/attachIntent");
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_NONE), "/perm/rules": ok([]) });
    openArea("/governance/classifications/t-pii", ADMIN);
    await userEvent.click(await within(await screen.findByRole("region", { name: "Effect" }, T)).findByRole("link", { name: "Attach in Data" }, T));
    expect(attachIntent()).toBe("t-pii");
  });

  it("says a stored rule is not enforced yet, and why, once the server says so", async () => {
    serve({
      "/tags": ok([PII]),
      "/classifications/t-pii/usage": ok(USAGE_TWO),
      "/perm/rules": ok([HASH]),
      "/perm/rules/enforcement?ids=r1": ok([{ ruleId: "r1", enforcedBy: 0, notEnforced: "no_engine" }]),
    });
    openArea("/governance/classifications/t-pii", ADMIN);
    await waitFor(() => expect(strip()).toHaveTextContent("Mask · Hash: Hash email"), T);
    await waitFor(() => expect(strip()).toHaveTextContent("Not enforced yet — no engine attached"), T);
  });

  it("says a rule enforced in one catalog but not another is partly enforced, and where not (live check D2)", async () => {
    serve({
      "/tags": ok([PII]),
      "/classifications/t-pii/usage": ok(USAGE_TWO),
      "/perm/rules": ok([HASH]),
      "/perm/rules/enforcement?ids=r1": ok([
        {
          ruleId: "r1",
          enforcedBy: 1,
          notEnforced: null,
          catalogs: [
            { catalogId: "c-a", catalogName: "sales", engines: [{ id: "e-1", name: "trino-a", engineType: "trino" }], notEnforced: null },
            { catalogId: "c-b", catalogName: "demo_warehouse", engines: [], notEnforced: "no_engine" },
          ],
        },
      ]),
    });
    openArea("/governance/classifications/t-pii", ADMIN);
    await waitFor(() => expect(strip()).toHaveTextContent("Partly enforced — Not enforced in demo_warehouse — no engine attached."), T);
    expect(strip()).not.toHaveTextContent("Not enforced yet");
  });

  it("says nothing about enforcement for an enforced rule, or while the answer is unknown", async () => {
    serve({
      "/tags": ok([PII]),
      "/classifications/t-pii/usage": ok(USAGE_TWO),
      "/perm/rules": ok([NULLIFY, HASH]),
      "/perm/rules/enforcement?ids=r1,r2": ok([
        { ruleId: "r1", enforcedBy: 1, notEnforced: null },
        { ruleId: "r2", enforcedBy: 0, notEnforced: "no_profile" },
      ]),
    });
    openArea("/governance/classifications/t-pii", ADMIN);
    // One of two not enforced: said as a count, never as the rule shown first.
    await waitFor(() => expect(strip()).toHaveTextContent("Not enforced yet — 1 of 2 rules: its engines have no policy profile"), T);

    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_TWO), "/perm/rules": ok([HASH]), "/perm/rules/enforcement?ids=r1": "never" });
    openArea("/governance/classifications/t-pii", ADMIN);
    await waitFor(() => expect(screen.getAllByRole("region", { name: "Effect" })[1]).toHaveTextContent("Mask · Hash: Hash email"), T);
    expect(screen.getAllByRole("region", { name: "Effect" })[1]).not.toHaveTextContent("Not enforced");
  });

  it.each([
    [{ required_roles: ["cleared"] }, "Only holders of 1 role, and platform admins, may open data with it in egret."],
    [
      { required_roles: ["cleared"], required_groups: ["g1", "g2"] },
      "Only holders of 1 role or members of 2 groups, and platform admins, may open data with it in egret.",
    ],
  ])("states a clearance with roles (%j) without the engine caveat", async (over, sentence) => {
    serve({ "/tags": ok([{ ...PII, ...over }]), "/classifications/t-pii/usage": ok(USAGE_NONE), "/perm/rules": ok([]) });
    openArea("/governance/classifications/t-pii", ADMIN);
    expect(await within(await screen.findByRole("region", { name: "Effect" }, T)).findByText(sentence, {}, T)).toBeInTheDocument();
    expect(strip()).not.toHaveTextContent("Engines do not enforce it.");
  });

  it("promises the Data page's Add classification only to who may attach", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_NONE), "/perm/rules": ok([]) });
    openArea("/governance/classifications/t-pii", STEWARD);
    await waitFor(() => expect(strip()).toHaveTextContent("Attaching needs the catalog_registry:update permission."), T);
    expect(strip()).not.toHaveTextContent("use Add classification");
    expect(within(strip()).queryByRole("link", { name: "Attach in Data" })).not.toBeInTheDocument();
    expect(within(strip()).getByRole("link", { name: "Open Data" })).toHaveAttribute("href", "/governance/data");
  });

  it("tells a viewer who may attach how to", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_NONE), "/perm/rules": ok([]) });
    openArea("/governance/classifications/t-pii", DATA_EDITOR);
    await waitFor(() => expect(strip()).toHaveTextContent("Open a table or a column and use Add classification."), T);
    expect(within(strip()).getByRole("link", { name: "Attach in Data" })).toHaveAttribute("href", "/governance/data");
    expect(strip()).not.toHaveTextContent("Attaching needs");
  });

  it("never says it does nothing while a fact is unknown", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": "never", "/perm/rules": ok([]) });
    openArea("/governance/classifications/t-pii", ADMIN);
    await waitFor(() => expect(strip()).toHaveTextContent("No rule uses this classification yet."), T);
    expect(strip()).not.toHaveTextContent("Does nothing yet.");
  });

  it("gives each fact its own error, with Retry", async () => {
    serve({
      "/tags": ok([PII]),
      "/classifications/t-pii/usage": fail(500),
      "/perm/rules": fail(500),
    });
    openArea("/governance/classifications/t-pii", ADMIN);
    await waitFor(() => expect(strip()).toHaveTextContent("The rules could not be loaded."), T);
    expect(strip()).toHaveTextContent("The classification's usage could not be loaded.");
    expect(within(strip()).getAllByRole("button", { name: "Retry" })).toHaveLength(2);
    expect(strip()).not.toHaveTextContent("Does nothing yet.");
  });

  it("Add rule opens the rule dialog on this classification, previews on its key, and the strip shows the new rule", async () => {
    const user = userEvent.setup();
    const answers: Record<string, Answer> = {
      "/tags": ok([PII]),
      "/classifications/t-pii/usage": ok(USAGE_NONE),
      "/perm/rules": ok([]),
      "/governance/catalogs": ok([]),
      "POST /perm/preview": ok(PREVIEWED),
      "POST /perm/rules": ok({ ...HASH, kind: { type: "mask", mask_type: "MASK" }, name: "Mask PII" }),
    };
    const served = serve(answers);
    openArea("/governance/classifications/t-pii", ADMIN);
    await user.click(await within(await screen.findByRole("region", { name: "Effect" }, T)).findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("dialog", { name: "New rule" });
    expect(within(dialog).getByLabelText(/^Name/)).toHaveValue("Mask PII");
    await user.click(within(dialog).getByRole("button", { name: "Preview" }));
    await waitFor(() => expect(served.bodies["POST /perm/preview"]).toHaveLength(1));
    const body = served.bodies["POST /perm/preview"][0] as { rule: { selector: unknown; name: string } };
    expect(body.rule.selector).toEqual({ type: "tag", key: "pii" });
    expect(body.rule.name).toBe("Mask PII");
    const create = within(dialog).getByRole("button", { name: "Create" });
    await waitFor(() => expect(create).toHaveAttribute("aria-disabled", "false"));
    answers["/perm/rules"] = ok([{ ...HASH, kind: { type: "mask", mask_type: "MASK" }, name: "Mask PII" }]);
    await user.click(create);
    await waitFor(() => expect(strip()).toHaveTextContent("Mask · Redact: Mask PII"), T);
    expect(strip()).not.toHaveTextContent("No rule uses this classification yet.");
  });

  it("says which permission is missing instead of hiding facts", async () => {
    const served = serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_NONE) });
    openArea("/governance/classifications/t-pii", TAG_READER);
    await waitFor(() => expect(strip()).toHaveTextContent("Reading rules needs the policy:read permission."), T);
    expect(strip()).toHaveTextContent("Setting clearance needs the tag:manage permission.");
    expect(within(strip()).queryByRole("button", { name: "Add rule" })).not.toBeInTheDocument();
    expect(within(strip()).queryByRole("button", { name: "Set clearance" })).not.toBeInTheDocument();
    expect(served.asked).not.toContain("GET /perm/rules");
    // Rules are unknown: the strip does not claim the classification does nothing.
    expect(strip()).not.toHaveTextContent("Does nothing yet.");
  });

  it("offers Add rule only with policy:update", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_NONE), "/perm/rules": ok([]) });
    openArea("/governance/classifications/t-pii", STEWARD);
    await waitFor(() => expect(strip()).toHaveTextContent("No rule uses this classification yet."), T);
    expect(within(strip()).queryByRole("button", { name: "Add rule" })).not.toBeInTheDocument();
    expect(within(strip()).getByText("Adding a rule needs the policy:update permission.")).toBeInTheDocument();
  });

  it("refreshes after a clearance change on the Clearance tab", async () => {
    const user = userEvent.setup();
    const answers: Record<string, Answer> = {
      "/tags": ok([PII]),
      "/classifications/t-pii/usage": ok(USAGE_NONE),
      "/perm/rules": ok([]),
      "/access/roles": ok([
        { id: "r1", scope: "platform", name: "auditor", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
      ]),
      "/groups": ok([]),
      "/perm/clearances/t-pii/people": ok({ count: 0, people: [], truncated: false, enginesEnforce: false, onEnginesCount: 0 }),
      "PUT /tags/t-pii/access-policy": ok({ ok: true, skipped: [] }),
    };
    serve(answers);
    openArea("/governance/classifications/t-pii?tab=clearance", ADMIN);
    await waitFor(() => expect(strip()).toHaveTextContent("None: anyone who can read the data can open it."), T);
    answers["/tags"] = ok([{ ...PII, required_roles: ["auditor"] }]);
    await user.click(await screen.findByRole("combobox", { name: "Role to add" }, T));
    await user.click(await screen.findByRole("button", { name: "auditor" }));
    await user.click(screen.getByRole("button", { name: "Add clearance" }));
    await waitFor(
      () => expect(strip()).toHaveTextContent("Only holders of 1 role, and platform admins, may open data with it in egret."),
      T,
    );
  });
});
