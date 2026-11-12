import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { TrinoConfigDialog } from "../TrinoConfigDialog";

// The two files exactly as `GET /policies/services/{service}/trino-config`
// answers since it moved onto the shared Ranger plugin renderer (SP4d-4).
const FILES = [
  {
    name: "access-control.properties",
    content:
      "access-control.name=ranger\nranger.service.name=p1\nranger.plugin.config.resource=/etc/trino/ranger-trino-security.xml\n",
  },
  {
    name: "ranger-trino-security.xml",
    content:
      '<?xml version="1.0" encoding="UTF-8"?>\n<configuration>\n<property><name>ranger.plugin.trino.service.name</name><value>p1</value></property>\n<property><name>ranger.plugin.trino.policy.source.impl</name><value>org.apache.ranger.admin.client.RangerAdminRESTClient</value></property>\n</configuration>\n',
  },
];

const WARNING =
  "The download URL (http://localhost:8080) is a loopback or unspecified address: no engine can reach it.";
const getTrinoConfig = vi.fn(() => Promise.resolve({ mechanism: "ranger", files: FILES, warnings: [] as string[] }));
vi.mock("@/api/policies", () => ({ getTrinoConfig: () => getTrinoConfig() }));

describe("TrinoConfigDialog", () => {
  it("lists both files with their content and downloads each by name", async () => {
    const created: Blob[] = [];
    URL.createObjectURL = vi.fn((b: Blob) => {
      created.push(b);
      return "blob:x";
    });
    URL.revokeObjectURL = vi.fn();
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <TrinoConfigDialog open service="p1" onOpenChange={() => {}} />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(screen.getByText("ranger-trino-security.xml")).toBeInTheDocument());
    expect(screen.getByText("access-control.properties")).toBeInTheDocument();
    expect(screen.getByText(/ranger\.plugin\.config\.resource=\/etc\/trino/)).toBeInTheDocument();
    const buttons = screen.getAllByRole("button", { name: "Download" });
    expect(buttons).toHaveLength(2);
    fireEvent.click(buttons[1]);
    expect(created).toHaveLength(1);
    expect(await created[0].text()).toBe(FILES[1].content);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  // KI-149: the same warning the external engine's plugin configuration shows.
  it("shows the server's warnings above the files", async () => {
    getTrinoConfig.mockResolvedValueOnce({ mechanism: "ranger", files: FILES, warnings: [WARNING] });
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <TrinoConfigDialog open service="p2" onOpenChange={() => {}} />
      </QueryClientProvider>,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(WARNING);
    expect(screen.getByText("ranger-trino-security.xml")).toBeInTheDocument();
  });

  it("for an OPA enforcement point shows both OPA files, the plugin/opa description and the warnings verbatim", async () => {
    const LOG = "Do not enable opa.log-requests: every request carries egret.secret in input.context.properties, and the coordinator log would hold it.";
    const KEEP = "Do not remove opa.policy.batched-uri from access-control.properties: without it Trino sends one request per object.";
    getTrinoConfig.mockResolvedValueOnce({
      mechanism: "opa",
      files: [
        { name: "access-control.properties", content: "access-control.name=opa\n" },
        { name: "egret-opa-context.properties", content: "egret.engine=REPLACE\n" },
      ],
      warnings: [LOG, KEEP],
    });
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <TrinoConfigDialog open service="p-opa" onOpenChange={() => {}} />
      </QueryClientProvider>,
    );
    expect(await screen.findByText("egret-opa-context.properties")).toBeInTheDocument();
    expect(screen.getByText("access-control.properties")).toBeInTheDocument();
    expect(screen.getByText(LOG)).toBeInTheDocument();
    expect(screen.getByText(KEEP)).toBeInTheDocument();
    expect(
      screen.getByText(/Install these two files on a Trino 483 or later coordinator with plugin\/opa/),
    ).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/native ranger plugin/);
  });

  it("for a Ranger enforcement point keeps the Ranger description", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <TrinoConfigDialog open service="p3" onOpenChange={() => {}} />
      </QueryClientProvider>,
    );
    await screen.findByText("ranger-trino-security.xml");
    expect(document.body.textContent).toMatch(/native ranger plugin/);
    expect(document.body.textContent).not.toMatch(/plugin\/opa/);
  });

  it("claims no plugin while loading and says what to do when the read fails", async () => {
    getTrinoConfig.mockImplementationOnce(() => new Promise(() => {}));
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { unmount } = render(
      <QueryClientProvider client={qc}>
        <TrinoConfigDialog open service="p-load" onOpenChange={() => {}} />
      </QueryClientProvider>,
    );
    expect(await screen.findByText("Loading the configuration files.")).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/ranger plugin/i);
    unmount();

    getTrinoConfig.mockRejectedValueOnce(new Error("boom"));
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <TrinoConfigDialog open service="p-err" onOpenChange={() => {}} />
      </QueryClientProvider>,
    );
    expect(await screen.findByText(/could not be read\. Close this dialog and open it again/)).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/native ranger plugin/);
  });
});
