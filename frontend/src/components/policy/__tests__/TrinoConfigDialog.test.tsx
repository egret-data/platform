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

vi.mock("@/api/policies", () => ({
  getTrinoConfig: vi.fn(() => Promise.resolve(FILES)),
}));

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
  });
});
