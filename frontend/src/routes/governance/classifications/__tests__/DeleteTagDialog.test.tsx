import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import type { RuleView, TagUsage } from "@/api/generated/model";
import { fail, ok, openArea, serve, STEWARD, tag, TAG_MANAGER, where, type Answer } from "./harness";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() } }));

beforeEach(() => vi.clearAllMocks());
afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const PII = tag({
  id: "t-pii",
  key: "pii",
  name: "PII",
  required_roles: ["cleared"],
  required_groups: ["g1", "g2"],
});
const PAGE = "/governance/classifications/t-pii";
const USAGE = "/classifications/t-pii/usage";

const USED: TagUsage = {
  tag: { id: "t-pii", key: "pii", name: "PII" },
  data: { tables: [], tableCount: 2, columnCount: 3, orphaned: 0 },
  bi: { readable: [{ kind: "chart", id: "c1", name: "C", workspaceSlug: "w1" }], unreadable: { count: 4 }, deleted: 0, checked: 5, remaining: 0 },
};

const rule = (id: string, key: string, tagName: string): RuleView => ({
  id,
  name: `rule-${id}`,
  kind: { type: "mask", mask_type: "MASK_HASH" },
  selector: { type: "tag", key },
  exempt: { include: [], exclude: [] },
  tag_name: tagName,
});
// Two rules select this tag's key; one on another tag that shares the name does not block the delete.
const RULES = [rule("r1", "pii", "PII"), rule("r2", "pii", "PII"), rule("r3", "pii_v2", "PII")];

function open(extra: Record<string, Answer> = {}, as = STEWARD, url = PAGE) {
  const served = serve({ "/tags": ok([PII]), [USAGE]: ok(USED), "/perm/rules": ok(RULES), ...extra });
  openArea(url, as);
  return served;
}

async function openDialog(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: "Delete" }, T));
  return screen.findByRole("alertdialog", {}, T);
}

describe("Delete a classification (CL14)", () => {
  it("states every consequence and the refusal sentence", async () => {
    const user = userEvent.setup();
    open();
    const dialog = await openDialog(user);
    await waitFor(() => expect(dialog).toHaveTextContent("Removes it from 2 tables and 3 columns."));
    expect(dialog).toHaveTextContent("Removes it from 1 BI object, and 4 you cannot read.");
    expect(dialog).toHaveTextContent("Removes its clearance (1 role, 2 groups).");
    await waitFor(() =>
      expect(dialog).toHaveTextContent(
        "2 rules select it: the delete will be refused until they are deleted or retargeted.",
      ),
    );
  });

  it("says no rule selects it when none does, without the refusal", async () => {
    const user = userEvent.setup();
    open({ "/perm/rules": ok([rule("r3", "pii_v2", "PII")]) });
    const dialog = await openDialog(user);
    await waitFor(() => expect(dialog).toHaveTextContent("No rule selects it."));
    expect(dialog).not.toHaveTextContent(/will be refused/);
  });

  it("without policy:read says rules need it, and asks for none", async () => {
    const user = userEvent.setup();
    const served = open({}, TAG_MANAGER);
    const dialog = await openDialog(user);
    expect(await within(dialog).findByText("Rules: needs policy:read to count.", {}, T)).toBeInTheDocument();
    expect(served.asked).not.toContain("GET /perm/rules");
  });

  it("still opens when usage cannot be read, and says why", async () => {
    const user = userEvent.setup();
    open({ [USAGE]: fail(500, "database unavailable") });
    const dialog = await openDialog(user);
    expect(await within(dialog).findByText("Usage could not be read: database unavailable", {}, T)).toBeInTheDocument();
  });

  it("enables Delete only for the exact key, not the name", async () => {
    const user = userEvent.setup();
    open();
    const dialog = await openDialog(user);
    const confirm = within(dialog).getByRole("button", { name: "Delete" });
    const input = within(dialog).getByPlaceholderText("pii");
    await user.type(input, "PII");
    expect(confirm).toBeDisabled();
    await user.clear(input);
    await user.type(input, "pii");
    expect(confirm).toBeEnabled();
  });

  it("deletes and goes to the index without a toast", async () => {
    const user = userEvent.setup();
    const served = open({ "DELETE /tags/t-pii": { status: 204, body: null } });
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByPlaceholderText("pii"), "pii");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(where()).toBe("/governance/classifications"));
    expect(served.asked).toContain("DELETE /tags/t-pii");
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("a 409 toasts the refusal, closes the dialog and returns focus to Delete", async () => {
    const user = userEvent.setup();
    const sentence = "this tag is still selected by a permission rule (perm.rules) — delete or retarget that rule first";
    open({ "DELETE /tags/t-pii": fail(409, sentence) });
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByPlaceholderText("pii"), "pii");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    const [message, opts] = vi.mocked(toast.error).mock.calls[0] as unknown as [string, { description: string }];
    expect(message).toBe(sentence);
    expect(opts.description).toBe('The "Rules" tab on this page lists the rules that use this classification.');
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("button", { name: "Delete" })).toHaveFocus());
    expect(where()).toBe(PAGE);
  });

  it("hides Delete without tag:manage", async () => {
    open({}, { user_id: "x", email: "x@e", platform_roles: [], permissions: ["tag:read", "policy:read"] });
    await screen.findByRole("heading", { level: 1, name: "PII" }, T);
    expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
  });

  // FE-ACTION-2: from the index the delete stays on the page, so a toast confirms it.
  it("deleting from the index stays there, closes the dialog and toasts", async () => {
    const user = userEvent.setup();
    const served = open({ "DELETE /tags/t-pii": { status: 204, body: null } }, STEWARD, "/governance/classifications");
    await user.click(await screen.findByRole("button", { name: "Actions for PII" }, T));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog", {}, T);
    await user.type(within(dialog).getByPlaceholderText("pii"), "pii");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Classification PII deleted."));
    expect(served.asked).toContain("DELETE /tags/t-pii");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(where()).toBe("/governance/classifications");
  });

  it("a 409 from the index is toasted too", async () => {
    const user = userEvent.setup();
    open({ "DELETE /tags/t-pii": fail(409, "still selected") }, STEWARD, "/governance/classifications");
    await user.click(await screen.findByRole("button", { name: "Actions for PII" }, T));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog", {}, T);
    await user.type(within(dialog).getByPlaceholderText("pii"), "pii");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("is offered from the index row menu too", async () => {
    const user = userEvent.setup();
    open({}, STEWARD, "/governance/classifications");
    await user.click(await screen.findByRole("button", { name: "Actions for PII" }, T));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog", {}, T);
    await waitFor(() => expect(dialog).toHaveTextContent("Removes it from 2 tables and 3 columns."));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("button", { name: "Actions for PII" })).toHaveFocus());
  });
});
