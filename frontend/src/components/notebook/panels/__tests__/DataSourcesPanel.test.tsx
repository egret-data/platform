import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

const runtimeMock = vi.fn();
vi.mock("@/api/notebooks", () => ({
  useNotebookRuntime: () => runtimeMock(),
}));

import { DataSourcesPanel } from "../DataSourcesPanel";

describe("DataSourcesPanel", () => {
  it("lists enabled engines from the runtime", () => {
    runtimeMock.mockReturnValue({
      data: {
        username: "u",
        engines: [
          { id: "e1", name: "Warehouse Trino", engine: "trino", enabled: true },
          { id: "e2", name: "Disabled SR", engine: "starrocks", enabled: false },
        ],
      },
    });
    render(<DataSourcesPanel ws="demo" />);
    expect(screen.getByText("Warehouse Trino")).toBeInTheDocument();
    expect(screen.queryByText("Disabled SR")).not.toBeInTheDocument();
  });

  it("shows an empty hint when there are no engines", () => {
    runtimeMock.mockReturnValue({ data: { username: "u", engines: [] } });
    render(<DataSourcesPanel ws="demo" />);
    expect(screen.getByText(/no data sources/i)).toBeInTheDocument();
  });
});
