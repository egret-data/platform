import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ADMIN, fail, ok, openArea, serve, STEWARD, tag, TAG_READER, where, type Answer } from "./harness";

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
    expect(strip()).toHaveTextContent("Only 2 groups may open data with it");
    expect(strip()).toHaveTextContent("2 tables · 3 columns · 0 BI objects");
    expect(strip()).not.toHaveTextContent("Does nothing yet.");
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

  it("Add rule opens the rule dialog on this classification", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_NONE), "/perm/rules": ok([]), "/governance/catalogs": ok([]) });
    openArea("/governance/classifications/t-pii", ADMIN);
    await userEvent.click(await within(await screen.findByRole("region", { name: "Effect" }, T)).findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("dialog", { name: "New rule" });
    expect(within(dialog).getByLabelText(/^Name/)).toHaveValue("Mask PII");
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
    await waitFor(() => expect(strip()).toHaveTextContent("Only 1 role may open data with it"), T);
  });
});
