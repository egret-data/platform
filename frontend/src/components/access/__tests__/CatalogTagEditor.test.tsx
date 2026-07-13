import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Tag } from "@/api/tags";
import type { ResourceTagAssignment } from "@/api/catalogTags";

const catalogTagsMock = vi.fn();
const resourceTagsMock = vi.fn();
const assignMutate = vi.fn();
const unassignMutate = vi.fn();

vi.mock("@/api/tags", () => ({
  useTags: () => catalogTagsMock(),
}));

vi.mock("@/api/catalogTags", () => ({
  useResourceTags: (...args: unknown[]) => resourceTagsMock(...args),
  useAssignResourceTag: () => ({ mutate: assignMutate, isPending: false }),
  useUnassignResourceTag: () => ({ mutate: unassignMutate, isPending: false }),
}));

import { CatalogTagEditor } from "@/components/access/CatalogTagEditor";

const tag = (overrides: Partial<Tag>): Tag => ({
  id: "t1",
  key: "pii",
  name: "PII",
  description: "",
  color: "#e11d48",
  assignment_count: 0,
  created_at: "2026-01-01T00:00:00Z",
  ...overrides,
});

const assignment = (overrides: Partial<ResourceTagAssignment>): ResourceTagAssignment => ({
  schema: "eng",
  table: "orders",
  column: "",
  tag: tag({}),
  ...overrides,
});

function renderEditor(props: {
  warehouseId?: string;
  schema?: string;
  table?: string;
  column?: string;
} = {}) {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <CatalogTagEditor
        warehouseId={props.warehouseId ?? "wh1"}
        schema={props.schema ?? "eng"}
        table={props.table ?? "orders"}
        column={props.column}
      />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  catalogTagsMock.mockReset().mockReturnValue({ data: [], isLoading: false });
  resourceTagsMock.mockReset().mockReturnValue({ data: [], isLoading: false });
  assignMutate.mockReset();
  unassignMutate.mockReset();
});

describe("CatalogTagEditor", () => {
  it("renders the resource's current tags as chips; removing calls unassign with resource coords", async () => {
    resourceTagsMock.mockReturnValue({
      data: [
        assignment({ column: "", tag: tag({ id: "t1", name: "PII" }) }),
        assignment({ column: "email", tag: tag({ id: "t2", name: "Sensitive" }) }),
      ],
      isLoading: false,
    });
    catalogTagsMock.mockReturnValue({
      data: [tag({ id: "t1", name: "PII" }), tag({ id: "t2", name: "Sensitive" })],
      isLoading: false,
    });
    const user = userEvent.setup();
    renderEditor(); // table-level (no column)

    expect(screen.getByText("PII")).toBeInTheDocument();
    expect(screen.queryByText("Sensitive")).not.toBeInTheDocument();
    await user.click(screen.getByLabelText("Remove PII"));
    expect(unassignMutate).toHaveBeenCalledWith({
      schema: "eng",
      table: "orders",
      column: "",
      tag_id: "t1",
    });
  });

  it("renders only the matching column's tags when column is set", () => {
    resourceTagsMock.mockReturnValue({
      data: [
        assignment({ column: "", tag: tag({ id: "t1", name: "PII" }) }),
        assignment({ column: "email", tag: tag({ id: "t2", name: "Sensitive" }) }),
      ],
      isLoading: false,
    });
    catalogTagsMock.mockReturnValue({
      data: [tag({ id: "t1", name: "PII" }), tag({ id: "t2", name: "Sensitive" })],
      isLoading: false,
    });
    renderEditor({ column: "email" });

    expect(screen.getByText("Sensitive")).toBeInTheDocument();
    expect(screen.queryByText("PII")).not.toBeInTheDocument();
  });

  it("the add-select lists catalog tags not already assigned to the resource; picking one assigns it", async () => {
    resourceTagsMock.mockReturnValue({
      data: [assignment({ column: "", tag: tag({ id: "t1", name: "PII" }) })],
      isLoading: false,
    });
    catalogTagsMock.mockReturnValue({
      data: [tag({ id: "t1", name: "PII" }), tag({ id: "t2", name: "Sensitive" })],
      isLoading: false,
    });
    const user = userEvent.setup();
    renderEditor();

    await user.click(screen.getByLabelText("Add tag"));
    expect(screen.queryByRole("option", { name: "PII" })).toBeNull();
    await user.click(await screen.findByRole("option", { name: "Sensitive" }));

    expect(assignMutate).toHaveBeenCalledWith({
      schema: "eng",
      table: "orders",
      column: "",
      tag_id: "t2",
    });
  });

  it("sends the column coordinate when tagging a column", async () => {
    resourceTagsMock.mockReturnValue({ data: [], isLoading: false });
    catalogTagsMock.mockReturnValue({ data: [tag({ id: "t1", name: "PII" })], isLoading: false });
    const user = userEvent.setup();
    renderEditor({ column: "email" });

    await user.click(screen.getByLabelText("Add tag"));
    await user.click(await screen.findByRole("option", { name: "PII" }));

    expect(assignMutate).toHaveBeenCalledWith({
      schema: "eng",
      table: "orders",
      column: "email",
      tag_id: "t1",
    });
  });

  it("shows a muted hint when the catalog has no tags defined", () => {
    resourceTagsMock.mockReturnValue({ data: [], isLoading: false });
    catalogTagsMock.mockReturnValue({ data: [], isLoading: false });
    renderEditor();

    expect(
      screen.getByText(/create them in Control Plane.*Classifications/i),
    ).toBeInTheDocument();
  });
});
