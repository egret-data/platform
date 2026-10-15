import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import BackfillDialog from "@/components/workflows/triggers/BackfillDialog";
import { FakeWorkflowServer, FLOW_ID, WS } from "@/routes/workflows/__tests__/fakeWorkflowServer";
import { flowTriggers, scheduleEntry } from "./triggerFixtures";

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
const dialog = () => screen.getByRole("dialog");

describe("BackfillDialog (W7f §4.3)", () => {
  it("sends the window in UTC, the inputs the form holds, and the labels in order", async () => {
    const { onClose } = renderDialog();
    fireEvent.change(screen.getByLabelText("Start"), { target: { value: "2020-01-01T00:00:00" } });
    fireEvent.change(screen.getByLabelText("End"), { target: { value: "2020-01-01T01:00:00" } });
    await userEvent.type(screen.getByRole("textbox", { name: /^req/ }), "x");
    await userEvent.click(screen.getByRole("button", { name: "Add label" }));
    await userEvent.type(screen.getByLabelText("Key"), "team");
    await userEvent.type(screen.getByLabelText("Value"), "ops");
    await userEvent.click(screen.getByRole("button", { name: "Start backfill" }));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    const [post] = server.callsTo("POST", createPath);
    expect(post.body).toEqual({
      start: new Date("2020-01-01T00:00:00").toISOString(),
      end: new Date("2020-01-01T01:00:00").toISOString(),
      inputs: { region: "eu", req: "x" },
      labels: [{ key: "team", value: "ops" }],
    });
  });

  it("keeps the submit off with its reason until the required input is filled", async () => {
    renderDialog();
    fireEvent.change(screen.getByLabelText("Start"), { target: { value: "2020-01-01T00:00:00" } });
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
    fireEvent.change(screen.getByLabelText("Start"), { target: { value: "1990-01-01T00:00:00" } });
    await userEvent.type(screen.getByRole("textbox", { name: /^req/ }), "x");
    await userEvent.click(screen.getByRole("button", { name: "Start backfill" }));

    expect(await within(dialog()).findByText("The start is more than ten years back; check the year")).toBeInTheDocument();
    expect(screen.getByLabelText("Start")).toHaveValue("1990-01-01T00:00");
    expect(screen.getByRole("textbox", { name: /^req/ })).toHaveValue("x");
    expect(onClose).not.toHaveBeenCalled();
  });

  it("a 409 backfill_exists closes the form and says why in a toast", async () => {
    server.backfillCreateReply = {
      status: 409,
      body: { error: "This trigger already has a backfill; pause or delete it first", code: "backfill_exists" },
    };
    const { onClose } = renderDialog();
    fireEvent.change(screen.getByLabelText("Start"), { target: { value: "2020-01-01T00:00:00" } });
    await userEvent.type(screen.getByRole("textbox", { name: /^req/ }), "x");
    await userEvent.click(screen.getByRole("button", { name: "Start backfill" }));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(toast.error).toHaveBeenCalledWith(expect.stringContaining("already has a backfill"));
  });

  it("a required FILE input keeps the submit off, saying a backfill cannot carry it", () => {
    renderDialog(FILE_REQUIRED);
    expect(screen.getByRole("button", { name: "Start backfill" })).toBeDisabled();
    expect(within(dialog()).getByText("A FILE input cannot be backfilled")).toBeInTheDocument();
  });
});
