import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import type { RuleView, TagUsage } from "@/api/generated/model";
import { ADMIN, fail, ok, openArea, serve, STEWARD, tag, TAG_MANAGER, where, type Answer } from "./harness";

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

/** Only the rule on another classification with the same name: nothing blocks the delete. */
const FREE = [rule("r3", "pii_v2", "PII")];
const HASH_RULE: RuleView = { ...rule("r1", "pii", "PII"), name: "Hash email" };

function open(extra: Record<string, Answer> = {}, as = STEWARD, url = PAGE) {
  const served = serve({ "/tags": ok([PII]), [USAGE]: ok(USED), "/perm/rules": ok(FREE), ...extra });
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
    open({ "/perm/rules": ok(RULES) });
    const dialog = await openDialog(user);
    await waitFor(() => expect(dialog).toHaveTextContent("Removes it from 2 tables and 3 columns."));
    expect(dialog).toHaveTextContent("Removes it from 1 BI object, and 4 you cannot read.");
    expect(dialog).toHaveTextContent("Removes its clearance (1 role, 2 groups).");
    await waitFor(() => expect(dialog).toHaveTextContent("Delete or change these rules first:"));
    expect(within(dialog).getByRole("link", { name: "rule-r1" })).toHaveAttribute("href", "/governance/rules/r1?from=classification:t-pii");
    expect(within(dialog).getByRole("link", { name: "rule-r2" })).toBeInTheDocument();
    // A rule on another classification with the same name does not block the delete.
    expect(within(dialog).queryByRole("link", { name: "rule-r3" })).not.toBeInTheDocument();
  });

  it("says no rule selects it when none does, without the refusal", async () => {
    const user = userEvent.setup();
    open();
    const dialog = await openDialog(user);
    await waitFor(() => expect(dialog).toHaveTextContent("No rule uses it."));
    expect(dialog).not.toHaveTextContent(/these rules first/);
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

  it("asks for the classification's NAME, not its key, in any case", async () => {
    const user = userEvent.setup();
    const CARD = tag({ id: "t-card", key: "card", name: "Card numbers" });
    serve({ "/tags": ok([CARD]), "/classifications/t-card/usage": ok(USED), "/perm/rules": ok([]) });
    openArea("/governance/classifications/t-card", STEWARD);
    const dialog = await openDialog(user);
    expect(within(dialog).getByText(/to confirm/)).toHaveTextContent("Type Card numbers to confirm");
    const confirm = within(dialog).getByRole("button", { name: "Delete" });
    const input = within(dialog).getByRole("textbox");
    await user.type(input, "card");
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await user.clear(input);
    await user.type(input, "card NUMBERS");
    expect(confirm).not.toHaveAttribute("aria-disabled");
  });

  it("until the name is typed, Delete is aria-disabled, says why, stays focusable and a click does nothing (live check D4)", async () => {
    const user = userEvent.setup();
    const served = open({ "DELETE /tags/t-pii": { status: 204, body: null } });
    const dialog = await openDialog(user);
    await waitFor(() => expect(dialog).toHaveTextContent("No rule uses it."));
    const confirm = within(dialog).getByRole("button", { name: "Delete" });
    // Not native `disabled`: it stays in the tab order and names its reason (FE-ACTION-4).
    expect(confirm).not.toHaveAttribute("disabled");
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    expect(confirm).toHaveAccessibleDescription("Type the classification's name to enable Delete.");
    expect(within(dialog).getByText("Type the classification's name to enable Delete.")).toBeInTheDocument();
    await user.click(confirm);
    expect(served.asked).not.toContain("DELETE /tags/t-pii");
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    expect(where()).toBe(PAGE);

    await user.type(within(dialog).getByRole("textbox"), "pii");
    expect(confirm).not.toHaveAttribute("aria-disabled");
    expect(confirm).not.toHaveAccessibleDescription();
  });

  it("while the rules are being read, Delete waits with that reason and nothing to type", async () => {
    const user = userEvent.setup();
    const answers: Record<string, Answer> = { "/tags": ok([PII]), [USAGE]: ok(USED), "/perm/rules": "never" };
    serve(answers);
    openArea(PAGE, STEWARD);
    const dialog = await openDialog(user);
    const del = within(dialog).getByRole("button", { name: "Delete" });
    expect(del).toHaveAttribute("aria-disabled", "true");
    expect(del).toHaveAccessibleDescription("Checking rules…");
    expect(within(dialog).queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("when the rules cannot be read, the name is asked for and the server decides", async () => {
    const user = userEvent.setup();
    open({ "/perm/rules": fail(500, "down") });
    const dialog = await openDialog(user);
    expect(await within(dialog).findByRole("textbox", {}, T)).toBeInTheDocument();
    // Waiting only for the name, not for the rules.
    expect(within(dialog).getByRole("button", { name: "Delete" })).toHaveAccessibleDescription(
      "Type the classification's name to enable Delete.",
    );
  });

  it("cannot delete while rules use it: Delete says why and links each rule", async () => {
    const user = userEvent.setup();
    open({ "/perm/rules": ok([HASH_RULE]) }, ADMIN);
    const dialog = await openDialog(user);
    const del = within(dialog).getByRole("button", { name: "Delete" });
    await waitFor(() => expect(del).toHaveAttribute("aria-disabled", "true"));
    expect(del).toHaveAccessibleDescription(/Delete or change these rules first/);
    expect(within(dialog).getByRole("link", { name: "Hash email" })).toHaveAttribute(
      "href",
      "/governance/rules/r1?from=classification:t-pii",
    );
    expect(within(dialog).queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("a tag_in_use refusal without policy:read toasts the count and the rules' names, never perm.rules", async () => {
    const user = userEvent.setup();
    open(
      {
        "DELETE /tags/t-pii": {
          status: 409,
          body: {
            error: "Rules still use this classification: Hash email. Delete them or change what they apply to first. (perm.rules)",
            code: "tag_in_use",
            rules: [{ id: "r1", name: "Hash email" }],
          },
        },
      },
      TAG_MANAGER,
      "/governance/classifications",
    );
    await user.click(await screen.findByRole("button", { name: "Actions for PII" }, T));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog", {}, T);
    await user.type(within(dialog).getByRole("textbox"), "PII");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("PII is still used by 1 rule.", expect.anything()));
    expect(JSON.stringify(vi.mocked(toast.error).mock.calls)).not.toContain("perm.rules");
    expect(JSON.stringify(vi.mocked(toast.error).mock.calls)).toContain("Hash email");
  });

  it("a tag_in_use refusal for a policy:read holder offers Show rules", async () => {
    const user = userEvent.setup();
    open(
      { "DELETE /tags/t-pii": { status: 409, body: { error: "x", code: "tag_in_use", rules: [{ id: "r1", name: "a" }, { id: "r2", name: "b" }] } } },
      STEWARD,
      "/governance/classifications",
    );
    await user.click(await screen.findByRole("button", { name: "Actions for PII" }, T));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog", {}, T);
    await user.type(within(dialog).getByRole("textbox"), "pii");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    const [message, opts] = vi.mocked(toast.error).mock.calls[0] as unknown as [string, { action: { label: string; onClick: () => void } }];
    expect(message).toBe("PII is still used by 2 rules.");
    expect(opts.action.label).toBe("Show rules");
    opts.action.onClick();
    await waitFor(() => expect(where()).toBe("/governance/classifications/t-pii?tab=rules"));
  });

  it("deletes and goes to the index without a toast", async () => {
    const user = userEvent.setup();
    const served = open({ "DELETE /tags/t-pii": { status: 204, body: null } });
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole("textbox"), "pii");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(where()).toBe("/governance/classifications"));
    expect(served.asked).toContain("DELETE /tags/t-pii");
    expect(toast.success).not.toHaveBeenCalled();
    // Focus lands on the index's heading, never on <body>.
    await waitFor(() => expect(screen.getByRole("heading", { level: 1, name: "Classifications" })).toHaveFocus(), T);
    // The deleted classification's own reads are dropped, not refetched into a 404.
    const afterDelete = served.asked.slice(served.asked.indexOf("DELETE /tags/t-pii") + 1);
    expect(afterDelete.filter((k) => k.includes("t-pii"))).toEqual([]);
  });

  it("an uncoded failure toasts the server's message, closes the dialog and returns focus to Delete", async () => {
    const user = userEvent.setup();
    const sentence = "database unavailable";
    open({ "DELETE /tags/t-pii": fail(500, sentence) });
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole("textbox"), "pii");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    expect(vi.mocked(toast.error).mock.calls[0][0]).toBe(sentence);
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
    await user.type(within(dialog).getByRole("textbox"), "pii");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Classification PII deleted."));
    expect(served.asked).toContain("DELETE /tags/t-pii");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(where()).toBe("/governance/classifications");
  });

  it("after a delete from the index, focus leaves the vanished row for New classification, and is never stolen", async () => {
    const user = userEvent.setup();
    const OTHER = tag({ id: "t-oth", key: "other", name: "Other" });
    const answers: Record<string, Answer> = {
      "/tags": ok([PII, OTHER]),
      [USAGE]: ok(USED),
      "/perm/rules": ok(FREE),
      "DELETE /tags/t-pii": { status: 204, body: null },
    };
    serve(answers);
    openArea("/governance/classifications", STEWARD);
    await user.click(await screen.findByRole("button", { name: "Actions for PII" }, T));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog", {}, T);
    await user.type(within(dialog).getByRole("textbox"), "pii");
    answers["/tags"] = ok([OTHER]);
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Actions for PII" })).not.toBeInTheDocument(), T);
    // The header's New classification (the empty state has none: the list still holds Other).
    await waitFor(() => expect(screen.getByRole("button", { name: "New classification" })).toHaveFocus(), T);
  });

  it("after a delete from the index, focus the user moved elsewhere stays there", async () => {
    const user = userEvent.setup();
    const OTHER = tag({ id: "t-oth", key: "other", name: "Other" });
    const answers: Record<string, Answer> = {
      "/tags": ok([PII, OTHER]),
      [USAGE]: ok(USED),
      "/perm/rules": ok(FREE),
      "DELETE /tags/t-pii": { status: 204, body: null },
    };
    serve(answers);
    // The delete answers only once the user has moved on.
    const served = globalThis.fetch;
    let answerDelete = () => {};
    vi.stubGlobal("fetch", (input: RequestInfo | URL, init?: RequestInit) =>
      init?.method === "DELETE"
        ? new Promise<Response>((resolve) => (answerDelete = () => resolve(served(input, init))))
        : served(input, init),
    );
    openArea("/governance/classifications", STEWARD);
    await user.click(await screen.findByRole("button", { name: "Actions for PII" }, T));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog", {}, T);
    await user.type(within(dialog).getByRole("textbox"), "pii");
    answers["/tags"] = ok([OTHER]);
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument(), T);
    const search = screen.getByRole("searchbox", { name: "Search classifications" });
    await waitFor(() => expect(screen.getByRole("button", { name: "Actions for PII" })).toHaveFocus(), T);
    search.focus();
    answerDelete();
    await waitFor(() => expect(screen.queryByRole("button", { name: "Actions for PII" })).not.toBeInTheDocument(), T);
    await new Promise((r) => setTimeout(r, 50));
    expect(search).toHaveFocus();
  });

  it("a 409 from the index is toasted too", async () => {
    const user = userEvent.setup();
    open({ "DELETE /tags/t-pii": fail(409, "still selected") }, STEWARD, "/governance/classifications");
    await user.click(await screen.findByRole("button", { name: "Actions for PII" }, T));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog", {}, T);
    await user.type(within(dialog).getByRole("textbox"), "pii");
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
