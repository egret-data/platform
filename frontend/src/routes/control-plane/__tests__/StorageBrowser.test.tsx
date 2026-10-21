import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { StorageObjectBrowser } from "@/routes/control-plane/StorageBrowser";
import * as api from "@/api/controlPlane";

vi.mock("@/api/controlPlane");

const CONN = { id: "c1", name: "sandbox-lake", bucket: "warehouse", key_prefix: "sandbox/" } as api.Storage;

function renderBrowser() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <StorageObjectBrowser storageId="c1" />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("StorageObjectBrowser", () => {
  beforeEach(() => {
    vi.mocked(api.getStorage).mockResolvedValue(CONN);
  });

  test("starts at the connection's key prefix (KI-129)", async () => {
    vi.mocked(api.browseStorageObjects).mockResolvedValue({
      root: "sandbox/",
      folders: ["inner/"],
      objects: [{ key: "s.csv", size: 12, lastModified: null }],
      hiddenSystemFolders: [],
    });
    renderBrowser();
    expect(await screen.findByRole("button", { name: "warehouse/sandbox" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Download s.csv" })).toHaveAttribute(
      "href",
      "/api/control-plane/storage/c1/download?key=s.csv",
    );
  });

  test("says which system folders it left out (KI-206)", async () => {
    vi.mocked(api.browseStorageObjects).mockResolvedValue({
      root: "",
      folders: ["demo/"],
      objects: [],
      hiddenSystemFolders: ["apps/", "query-results/"],
    });
    renderBrowser();
    expect(await screen.findByTestId("storage-hidden-system")).toHaveTextContent(
      /apps, query-results are not shown: .*Only a platform admin can open them\./,
    );
    expect(screen.queryByRole("button", { name: /query-results/ })).not.toBeInTheDocument();
  });
});
