import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

// Two workspace databases: one egret-provisioned (engine_id set), one self-service.
vi.mock("@/api/sqllab", () => ({
  useDatabases: () => ({
    data: [
      { id: "db-prov", name: "trino-prod", engine: "trino", engine_id: "eng-1" },
      { id: "db-self", name: "my-starrocks", engine: "starrocks" },
    ],
  }),
  useCatalogs: () => ({ data: [] }),
  useSchemas: () => ({ data: [] }),
  useTables: () => ({ data: [] }),
  useColumns: () => ({ data: [] }),
}));

import { DataPanel } from "@/components/sqllab/DataPanel";

function renderPanel() {
  return render(
    <DataPanel
      ws="eng"
      dbId={null}
      onDb={vi.fn()}
      onInsert={vi.fn()}
      onWords={vi.fn()}
      onCollapse={vi.fn()}
    />,
  );
}

test("tags egret-provisioned engines with a badge in the database picker", async () => {
  const user = userEvent.setup();
  renderPanel();

  // Open the database picker (first combobox in the panel).
  await user.click(screen.getAllByRole("combobox")[0]);

  // Both options are listed…
  const provisioned = await screen.findByRole("option", { name: /trino-prod/i });
  const selfService = await screen.findByRole("option", { name: /my-starrocks/i });

  // …but only the provisioned one carries the "egret" tag.
  expect(provisioned).toHaveTextContent("egret");
  expect(selfService).not.toHaveTextContent("egret");
});
