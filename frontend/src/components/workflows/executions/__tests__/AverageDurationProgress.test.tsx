import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AverageDurationProgress } from "@/components/workflows/executions/AverageDurationProgress";

interface TestExecution {
  state: string;
  history: string[];
  namespace: string;
  flow_key: string;
  started_at: string;
  ended_at: string | null;
  created_at: string;
  updated_at: string;
}

const START = Date.parse("2026-11-23T10:00:00.000Z");
const running: TestExecution = {
  state: "RUNNING",
  history: ["CREATED", "RUNNING"],
  namespace: "company.team",
  flow_key: "hello",
  started_at: new Date(START).toISOString(),
  ended_at: null,
  created_at: new Date(START).toISOString(),
  updated_at: new Date(START).toISOString(),
};

function serve(body: unknown) {
  const fetchMock = vi.fn(
    async (_url: string) => new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } }),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function setup(execution: TestExecution, nowMs: number) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AverageDurationProgress ws="acme" execution={execution} now={nowMs} />
    </QueryClientProvider>,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("AverageDurationProgress", () => {
  it("asks for the flow's average by namespace and flow KEY, and shows elapsed over average", async () => {
    const fetchMock = serve({ avgDurationMs: 100_000, count: 7 });
    setup(running, START + 40_000);
    const bar = await screen.findByRole("progressbar", { name: "Estimated progress" });
    expect(bar).toHaveAttribute("aria-valuenow", "40");
    expect(screen.getByText(/about 40%/)).toBeInTheDocument();
    expect(fetchMock.mock.calls[0][0]).toBe(
      "/api/ws/acme/workflows/executions/namespaces/company.team/flows/hello/average-duration",
    );
  });

  it("never passes 100% when the run outlasts the average", async () => {
    serve({ avgDurationMs: 10_000, count: 3 });
    setup(running, START + 60_000);
    const bar = await screen.findByRole("progressbar", { name: "Estimated progress" });
    expect(bar).toHaveAttribute("aria-valuenow", "100");
    expect(screen.getByText(/longer than usual/)).toBeInTheDocument();
  });

  it("shows nothing when there is no history to average", async () => {
    const fetchMock = serve({ avgDurationMs: null, count: 0 });
    setup(running, START + 5_000);
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByRole("progressbar")).toBeNull();
  });

  it("does not ask for a finished execution", async () => {
    const fetchMock = serve({ avgDurationMs: 100_000, count: 7 });
    setup({ ...running, state: "SUCCESS", ended_at: new Date(START + 1000).toISOString() }, START + 5_000);
    await new Promise((r) => setTimeout(r, 20));
    expect(fetchMock).not.toHaveBeenCalled();
    expect(screen.queryByRole("progressbar")).toBeNull();
  });
});
