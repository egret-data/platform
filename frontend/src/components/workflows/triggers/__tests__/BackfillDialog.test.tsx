import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import BackfillDialog from "@/components/workflows/triggers/BackfillDialog";
import { FakeWorkflowServer, FLOW_ID, WS } from "@/routes/workflows/__tests__/fakeWorkflowServer";
import { flowTriggers, scheduleEntry } from "./triggerFixtures";

// Start and End are the app's calendar (KI-276); its month grid loads lazily, so load it once here.
beforeAll(async () => {
  await import("@/components/ui/calendar");
}, 30_000);

/** The 1st of this month, local, at `time`: always in the past, so under the picker's `max` (now). */
const first = (time: string) => {
  const n = new Date();
  return new Date(`${n.getFullYear()}-${String(n.getMonth() + 1).padStart(2, "0")}-01T${time}`);
};

/** Opens the picker labelled `label`, picks the 1st of this month, and types `time` (HH:mm:ss). */
async function pickFirst(label: string, time: string) {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: label }));
  // The grid also shows the next month's first days, so the first match is this month's.
  await user.click(within(await screen.findByRole("grid")).getAllByRole("button", { name: /\b1st\b/ })[0]);
  const field = screen.getByLabelText("Time");
  await user.clear(field);
  await user.type(field, `${time}{Enter}`);
}

const SOURCE = `id: hello
namespace: company.team
inputs:
  - id: region
    type: STRING
    defaults: us
  - id: req
    type: STRING
  - id: upload
    type: FILE
    required: false
triggers:
  - id: daily
    type: io.kestra.plugin.core.trigger.Schedule
    cron: "*/5 * * * *"
    inputs:
      region: eu
tasks:
  - id: hello
    type: io.kestra.plugin.core.log.Log
    message: hi
`;

const FILE_REQUIRED = SOURCE.replace("    required: false", "    required: true");

let server: FakeWorkflowServer;
beforeEach(() => {
  for (const f of Object.values(toast)) f.mockReset();
  server = new FakeWorkflowServer();
  server.install();
  server.triggers = flowTriggers({ triggers: [scheduleEntry({ trigger_id: "daily" })] });
});
afterEach(() => vi.unstubAllGlobals());

function renderDialog(source = SOURCE, onClose = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <BackfillDialog
        ws={WS}
        flowUuid={FLOW_ID}
        trigger={{ ...scheduleEntry({ trigger_id: "daily" }), flow_uuid: FLOW_ID }}
        source={source}
        onClose={onClose}
      />
    </QueryClientProvider>,
  );
  return { onClose };
}

const createPath = `/api/ws/${WS}/workflows/flows/${FLOW_ID}/triggers/daily/backfill`;
// A picker left open is a second `dialog` (its popover), mounted after the backfill dialog.
const dialog = () => screen.getAllByRole("dialog")[0];

describe("BackfillDialog (W7f §4.3)", () => {
  it("sends the window in UTC, the inputs the form holds, and the labels in order", async () => {
    const { onClose } = renderDialog();
    await pickFirst("Start", "00:00:00");
    await pickFirst("End", "01:00:00");
    await userEvent.type(screen.getByRole("textbox", { name: /^req/ }), "x");
    await userEvent.click(screen.getByRole("button", { name: "Add label" }));
    await userEvent.type(screen.getByLabelText("Key"), "team");
    await userEvent.type(screen.getByLabelText("Value"), "ops");
    await userEvent.click(screen.getByRole("button", { name: "Start backfill" }));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    const [post] = server.callsTo("POST", createPath);
    expect(post.body).toEqual({
      start: first("00:00:00").toISOString(),
      end: first("01:00:00").toISOString(),
      inputs: { region: "eu", req: "x" },
      labels: [{ key: "team", value: "ops" }],
    });
  });

  it("keeps the submit off with its reason until the required input is filled", async () => {
    renderDialog();
    await pickFirst("Start", "00:00:00");
    const submit = screen.getByRole("button", { name: "Start backfill" });
    expect(submit).toBeDisabled();
    expect(within(dialog()).getByText("Fix the inputs")).toBeInTheDocument();
  });

  it("a refused start sits beside Start, and what was typed stays", async () => {
    server.backfillCreateReply = {
      status: 400,
      body: { error: "The start is more than ten years back; check the year", code: "backfill_start_too_early" },
    };
    const { onClose } = renderDialog();
    await pickFirst("Start", "00:00:00");
    await userEvent.type(screen.getByRole("textbox", { name: /^req/ }), "x");
    await userEvent.click(screen.getByRole("button", { name: "Start backfill" }));

    expect(await within(dialog()).findByText("The start is more than ten years back; check the year")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start" })).toHaveTextContent(/-01 00:00:00$/);
    expect(screen.getByRole("textbox", { name: /^req/ })).toHaveValue("x");
    expect(onClose).not.toHaveBeenCalled();
  });

  it("a 409 backfill_exists closes the form and says why in a toast", async () => {
    server.backfillCreateReply = {
      status: 409,
      body: { error: "This trigger already has a backfill; pause or delete it first", code: "backfill_exists" },
    };
    const { onClose } = renderDialog();
    await pickFirst("Start", "00:00:00");
    await userEvent.type(screen.getByRole("textbox", { name: /^req/ }), "x");
    await userEvent.click(screen.getByRole("button", { name: "Start backfill" }));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(toast.error).toHaveBeenCalledWith(expect.stringContaining("already has a backfill"));
  });

  it("prefills a BOOL input from the schedule's value, a boolean or its text", async () => {
    const source = SOURCE.replace(
      "  - id: upload\n",
      "  - id: loud\n    type: BOOL\n  - id: quiet\n    type: BOOL\n    defaults: true\n  - id: upload\n",
    ).replace("      region: eu\n", "      region: eu\n      loud: true\n      quiet: \"FALSE\"\n");
    const { onClose } = renderDialog(source);
    // `loud` has no default, so only the schedule can turn it on; `quiet`'s default is on, so only the schedule can turn it off.
    expect(screen.getByRole("switch", { name: /^loud/ })).toBeChecked();
    expect(screen.getByRole("switch", { name: /^quiet/ })).not.toBeChecked();
    await pickFirst("Start", "00:00:00");
    await userEvent.type(screen.getByRole("textbox", { name: /^req/ }), "x");
    await userEvent.click(screen.getByRole("button", { name: "Start backfill" }));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    const [post] = server.callsTo("POST", createPath);
    expect(post.body).toMatchObject({ inputs: { region: "eu", req: "x", loud: true, quiet: false } });
  });

  it("a required FILE input keeps the submit off, saying a backfill cannot carry it", () => {
    renderDialog(FILE_REQUIRED);
    expect(screen.getByRole("button", { name: "Start backfill" })).toBeDisabled();
    expect(within(dialog()).getByText("A FILE input cannot be backfilled")).toBeInTheDocument();
  });
});
