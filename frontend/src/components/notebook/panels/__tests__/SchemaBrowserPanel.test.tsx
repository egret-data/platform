import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const databasesMock = vi.fn();
vi.mock("@/api/sqllab", () => ({
  useDatabases: () => databasesMock(),
  useCatalogs: () => ({ isLoading: false, data: ["main"] }),
  useSchemas: () => ({ isLoading: false, data: ["public"] }),
  useTables: () => ({ isLoading: false, data: ["orders"] }),
  useColumns: () => ({
    isLoading: false,
    data: [{ name: "id", type: "bigint" }],
  }),
}));

import { SchemaBrowserPanel } from "../SchemaBrowserPanel";

function renderPanel() {
  const onInsertText = vi.fn();
  const onNewSqlCell = vi.fn();
  const onObjectsLoaded = vi.fn();
  render(
    <SchemaBrowserPanel
      ws="eng"
      onInsertText={onInsertText}
      onNewSqlCell={onNewSqlCell}
      onObjectsLoaded={onObjectsLoaded}
    />,
  );
  return { onInsertText, onNewSqlCell, onObjectsLoaded };
}

describe("SchemaBrowserPanel", () => {
  // One Trino database by default; the empty-state test overrides it locally.
  beforeEach(() => {
    databasesMock.mockReturnValue({
      isLoading: false,
      data: [{ id: "db1", name: "Warehouse", engine: "trino", engine_id: null }],
    });
  });

  it("renders databases and lazily reveals catalogs/schemas/tables on expand", async () => {
    const user = userEvent.setup();
    renderPanel();

    expect(screen.getByText("Warehouse")).toBeInTheDocument();
    // Deeper levels aren't fetched/rendered until their ancestor is expanded.
    expect(screen.queryByText("main")).not.toBeInTheDocument();

    await user.click(screen.getByText("Warehouse")); // expand database → catalogs
    expect(await screen.findByText("main")).toBeInTheDocument();

    await user.click(screen.getByText("main")); // expand catalog → schemas
    expect(await screen.findByText("public")).toBeInTheDocument();

    await user.click(screen.getByText("public")); // expand schema → tables
    expect(await screen.findByText("orders")).toBeInTheDocument();
  });

  it("clicking a table row creates a seeded SQL cell", async () => {
    const user = userEvent.setup();
    const { onNewSqlCell } = renderPanel();

    await user.click(screen.getByText("Warehouse"));
    await user.click(await screen.findByText("main"));
    await user.click(await screen.findByText("public"));
    await user.click(await screen.findByText("orders"));

    expect(onNewSqlCell).toHaveBeenCalledWith("SELECT * FROM main.public.orders LIMIT 100");
  });

  it("expanding a table reveals columns; clicking a column inserts its name", async () => {
    const user = userEvent.setup();
    const { onInsertText } = renderPanel();

    await user.click(screen.getByText("Warehouse"));
    await user.click(await screen.findByText("main"));
    await user.click(await screen.findByText("public"));

    // Expand the table via its chevron (a plain click on the row queries instead).
    const tableRow = await screen.findByTitle(/main\.public\.orders/i);
    const chevron = tableRow.querySelector('[role="button"]')!;
    await user.click(chevron);

    const columnRow = await screen.findByTitle("Insert id");
    await user.click(columnRow);

    expect(onInsertText).toHaveBeenCalledWith("id");
  });

  it("reports catalog/schema/table/column names via onObjectsLoaded as the tree expands", async () => {
    const user = userEvent.setup();
    const { onObjectsLoaded } = renderPanel();

    await user.click(screen.getByText("Warehouse"));
    // Catalog level loaded.
    expect(onObjectsLoaded).toHaveBeenCalledWith(["main"]);

    await user.click(await screen.findByText("main"));
    // Schema level loaded.
    expect(onObjectsLoaded).toHaveBeenCalledWith(["public"]);

    await user.click(await screen.findByText("public"));
    // Table level loaded: bare name + fully-qualified.
    expect(onObjectsLoaded).toHaveBeenCalledWith(["orders", "main.public.orders"]);

    const tableRow = await screen.findByTitle(/main\.public\.orders/i);
    const chevron = tableRow.querySelector('[role="button"]')!;
    await user.click(chevron);
    // Column level loaded.
    expect(await screen.findByTitle("Insert id")).toBeInTheDocument();
    expect(onObjectsLoaded).toHaveBeenCalledWith(["id"]);
  });

  it("shows an empty hint when there are no databases granted", () => {
    databasesMock.mockReturnValue({ isLoading: false, data: [] });
    renderPanel();
    expect(screen.getByText(/no databases granted/i)).toBeInTheDocument();
  });
});
