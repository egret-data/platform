import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import { FakeWorkflowServer } from "@/routes/workflows/__tests__/fakeWorkflowServer";
import { flowEntry, flowTriggers, otherEntry, scheduleEntry, webhookEntry } from "./triggerFixtures";
import { renderTriggersTab, TAB_PATH } from "./renderTriggersTab";
import { triggerBlock } from "@/components/workflows/triggers/triggerYaml";

const SECRET_PATH = "/workflows/v1/eng/executions/webhook/company.team/hello/s3cr3t-key";
const LATER = new Date(Date.now() + 25 * 60_000).toISOString();
const EARLIER = new Date(Date.now() - 5 * 60_000).toISOString();

let server: FakeWorkflowServer;

beforeEach(() => {
  for (const f of Object.values(toast)) f.mockReset();
  server = new FakeWorkflowServer();
  server.triggers = flowTriggers({
    triggers: [
      scheduleEntry(
        { runtime_disabled: true, effective_disabled: true },
        {
          cron: "0 9 * * *",
          timezone: "Asia/Ho_Chi_Minh",
          next_evaluation_date: "2026-10-16T02:00:00Z",
          last_error: "the scheduler could not claim the row",
          missed_ticks: { count: 4, more: false },
        },
      ),
      webhookEntry(),
      flowEntry({
        flow: {
          states: ["SUCCESS", "WARNING"],
          listening: true,
          not_listening_reason: null,
          depends_on: {
            mode: "ALL",
            min_satisfied: null,
            window: { kind: "duration", start: EARLIER, end: LATER },
            conditions: [
              { namespace: "company.team", flow_id: "extract", states: ["SUCCESS"], labels: null, when: null, met: true },
              { namespace: "company.team", flow_id: "load", states: ["SUCCESS"], labels: null, when: null, met: false },
            ],
          },
        },
      }),
      otherEntry(),
    ],
  });
});
afterEach(() => vi.unstubAllGlobals());

const drawer = (name: string) => screen.findByRole("dialog", { name: `Trigger ${name}` });

describe("TriggerDetailsDrawer: the URL", () => {
  it("opens from ?trigger= and closing removes it", async () => {
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=hook` });
    const d = await drawer("hook");
    expect(d).toBeInTheDocument();
    await userEvent.click(within(d).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByTestId("where")).toHaveTextContent(new RegExp(`^${TAB_PATH}$`));
  });

  it("opens from Details and from the trigger's id, which put it in the URL", async () => {
    renderTriggersTab(server);
    await userEvent.click(await screen.findByRole("link", { name: "Details of upstream" }));
    expect(await drawer("upstream")).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent(`${TAB_PATH}?trigger=upstream`);
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await userEvent.click(screen.getByRole("link", { name: "daily" }));
    expect(await drawer("daily")).toBeInTheDocument();
  });

  it("says a trigger the flow no longer declares is gone", async () => {
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=removed` });
    const d = await drawer("removed");
    expect(within(d).getByText("This trigger no longer exists in the flow; reload.")).toBeInTheDocument();
  });
});

describe("TriggerDetailsDrawer: per kind", () => {
  it("shows a schedule's facts, in the trigger's zone and the viewer's", async () => {
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=daily` });
    const d = await drawer("daily");
    expect(within(d).getByText("0 9 * * *")).toBeInTheDocument();
    expect(within(d).getByText("Asia/Ho_Chi_Minh")).toBeInTheDocument();
    // Switched off, it has no next run (KI-189): the stored date is not shown as one.
    expect(within(d).getByText("Next run").nextElementSibling).toHaveTextContent("None: the trigger does not fire (see its status)");
    expect(within(d).queryByText("(the trigger's timezone)")).toBeNull();
    expect(within(d).getByText("the scheduler could not claim the row")).toBeInTheDocument();
    expect(within(d).getByText("Missed ticks").nextElementSibling).toHaveTextContent("4");
    expect(within(d).getByText(/^Switched off/)).toBeInTheDocument();
  });

  it("shows a flow trigger's window and each condition, met or not", async () => {
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=upstream` });
    const d = await drawer("upstream");
    expect(within(d).getByText("SUCCESS, WARNING")).toBeInTheDocument();
    expect(within(d).getByText(/^duration: /)).toBeInTheDocument();
    expect(within(d).getByText(/closes in 25 minutes/)).toBeInTheDocument();
    const items = within(within(d).getByRole("list", { name: "Conditions" })).getAllByRole("listitem");
    expect(items.map((li) => li.textContent)).toEqual(["company.team.extract in SUCCESSMet", "company.team.load in SUCCESSNot met"]);
  });

  it("shows the next run of a schedule that fires, in the trigger's zone and the viewer's", async () => {
    server.triggers.triggers[0] = { ...server.triggers.triggers[0], runtime_disabled: false, effective_disabled: false };
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=daily` });
    const d = await drawer("daily");
    // 02:00 UTC is 09:00 in Ho Chi Minh City (UTC+7, no DST).
    // In `fullTimestamp`'s format, whatever the test machine's own zone.
    expect(within(d).getByText("(the trigger's timezone)").parentElement).toHaveTextContent(
      /^2026-10-16 09:00:00 \(the trigger's timezone\)$/,
    );
    expect(within(d).getByText(/\(your time, /)).toBeInTheDocument();
  });

  it("says when no window is open", async () => {
    const t = server.triggers.triggers[2];
    server.triggers.triggers[2] = { ...t, flow: { ...t.flow!, depends_on: { ...t.flow!.depends_on!, window: null } } };
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=upstream` });
    const d = await drawer("upstream");
    expect(within(d).getByText("No window is open")).toBeInTheDocument();
  });

  it("shows the trigger's YAML block, and opens the editor at it", async () => {
    const { onEdit } = renderTriggersTab(server, { at: `${TAB_PATH}?trigger=daily` });
    const d = await drawer("daily");
    expect(within(d).getByRole("figure", { name: "YAML of daily" }).querySelector("pre")!.textContent).toBe(
      'id: daily\ntype: io.kestra.plugin.core.trigger.Schedule\ncron: "*/5 * * * *"',
    );
    await userEvent.click(within(d).getByRole("button", { name: "Edit in the editor" }));
    expect(onEdit).toHaveBeenCalledWith("daily");
  });
});

describe("TriggerDetailsDrawer: the webhook path (D17)", () => {
  it("masks the key until Reveal, copies the path, and announces Copied", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=hook` });
    const d = await drawer("hook");
    const shown = within(d).getByTestId("webhook-path");
    expect(shown.textContent).not.toContain("s3cr3t-key");
    expect(shown.textContent).toBe("/workflows/v1/eng/executions/webhook/company.team/hello/••••••••");
    // The YAML block hides the same key: nowhere in the drawer before Reveal.
    const yaml = () => within(d).getByRole("figure", { name: "YAML of hook" }).querySelector("pre")!.textContent;
    expect(yaml()).toBe("id: hook\ntype: io.kestra.plugin.core.trigger.Webhook\nkey: ••••••••");
    expect(d.textContent).not.toContain("s3cr3t-key");
    const reveal = within(d).getByRole("button", { name: "Reveal" });
    expect(reveal).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(reveal);
    // One signal: pressed, under the same label.
    expect(within(d).getByRole("button", { name: "Reveal" })).toHaveAttribute("aria-pressed", "true");
    expect(within(d).getByTestId("webhook-path").textContent).toBe(SECRET_PATH);
    expect(yaml()).toBe("id: hook\ntype: io.kestra.plugin.core.trigger.Webhook\nkey: s3cr3t-key");
    await userEvent.click(within(d).getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(SECRET_PATH);
    const live = within(d).getByText("Copied");
    expect(live).toHaveAttribute("aria-live", "polite");
    // A path, never a URL built from the page's origin.
    expect(writeText.mock.calls[0][0]).not.toContain(window.location.origin);
    expect(within(d).getByText(/served by the egret backend, at whatever host your operator exposes for webhooks\.$/)).toBeInTheDocument();
  });

  it("adds that webhooks are off, and shows the key's error in place of the path", async () => {
    server.triggers = {
      ...server.triggers,
      webhooks_enabled: false,
      triggers: [webhookEntry({ webhook: { url_path: null, key_error: "the key renders to an empty string", unsupported: null } })],
    };
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=hook` });
    const d = await drawer("hook");
    expect(within(d).getByText("the key renders to an empty string")).toBeInTheDocument();
    expect(within(d).queryByTestId("webhook-path")).not.toBeInTheDocument();
    expect(within(d).queryByRole("button", { name: "Copy" })).not.toBeInTheDocument();
    expect(within(d).getByText(/Webhooks are off on this server\.$/)).toBeInTheDocument();
  });
});

describe("TriggerDetailsDrawer: another trigger starts hidden", () => {
  it("hides the key again when the drawer moves to another webhook", async () => {
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=hook` });
    const d = await drawer("hook");
    await userEvent.click(within(d).getByRole("button", { name: "Reveal" }));
    expect(d.textContent).toContain("s3cr3t-key");
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await userEvent.click(screen.getByRole("link", { name: "Details of hook" }));
    expect((await drawer("hook")).textContent).not.toContain("s3cr3t-key");
  });
});

describe("TriggerDetailsDrawer: the key is hidden in every shape of YAML", () => {
  const W = "io.kestra.plugin.core.trigger.Webhook";
  it.each([
    ["(a) a flow mapping", `triggers:\n  - { id: hook, type: ${W}, key: s3cr3t }\n`],
    ["(b) a quoted key name", `triggers:\n  - id: hook\n    type: ${W}\n    "key": s3cr3t\n`],
    ["(b) a single-quoted key name", `triggers:\n  - id: hook\n    type: ${W}\n    'key': s3cr3t\n`],
    ["(c) a quoted value over lines", `triggers:\n  - id: hook\n    type: ${W}\n    key: "abc\n      s3cr3t"\n`],
    ["(c) a plain value over lines", `triggers:\n  - id: hook\n    type: ${W}\n    key: abc\n      s3cr3t\n`],
    ["(d) a sequence at the key's indent", `triggers:\n  - id: hook\n    type: ${W}\n    key:\n    - s3cr3t\n`],
    ["a block that does not parse (all hidden)", `triggers:\n  - id: hook\n    type: ${W}\n    key: s3cr3t\n    bad: : :\n`],
  ])("%s", async (_shape, source) => {
    server.triggers = { ...server.triggers, triggers: [webhookEntry({ webhook: { url_path: null, key_error: "the key does not render", unsupported: null } })] };
    const block = triggerBlock(source, "hook");
    expect(block).toContain("s3cr3t");
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=hook`, source });
    const d = await drawer("hook");
    expect(d.textContent).not.toContain("s3cr3t");
    await userEvent.click(within(d).getByRole("button", { name: "Reveal" }));
    expect(within(d).getByRole("figure", { name: "YAML of hook" }).querySelector("pre")!.textContent).toBe(block);
  });

  it("says when the whole block is hidden", async () => {
    const source = `triggers:\n  - id: hook\n    type: ${W}\n    key: s3cr3t\n    bad: : :\n`;
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=hook`, source });
    const d = await drawer("hook");
    expect(within(d).getByText(/^The whole block is hidden/)).toBeInTheDocument();
  });
});

describe("TriggerDetailsDrawer: the path's key", () => {
  it("masks a key holding a slash, which the server sends as %2F in the last segment", async () => {
    server.triggers = {
      ...server.triggers,
      triggers: [webhookEntry({ webhook: { url_path: "/workflows/v1/eng/executions/webhook/company.team/hello/team%2Fs3cr3t", key_error: null, unsupported: null } })],
    };
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=hook` });
    const d = await drawer("hook");
    expect(within(d).getByTestId("webhook-path").textContent).toBe("/workflows/v1/eng/executions/webhook/company.team/hello/••••••••");
    expect(d.textContent).not.toContain("s3cr3t");
  });

  it("offers no Reveal when there is neither a path nor a block to reveal", async () => {
    server.triggers = { ...server.triggers, triggers: [webhookEntry({ webhook: { url_path: null, key_error: "no key is declared", unsupported: null } })] };
    renderTriggersTab(server, { at: `${TAB_PATH}?trigger=hook`, source: "id: hello\nnamespace: company.team\n" });
    const d = await drawer("hook");
    expect(within(d).getByText("no key is declared")).toBeInTheDocument();
    expect(within(d).queryByRole("button", { name: "Reveal" })).not.toBeInTheDocument();
  });
});
