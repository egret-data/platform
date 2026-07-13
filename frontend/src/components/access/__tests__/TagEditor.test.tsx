import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Tag } from "@/api/tags";

const objectTagsMock = vi.fn();
const catalogTagsMock = vi.fn();
const assignMutate = vi.fn();
const unassignMutate = vi.fn();

vi.mock("@/api/tags", () => ({
  useObjectTags: (...args: unknown[]) => objectTagsMock(...args),
  useTags: () => catalogTagsMock(),
  useAssignTag: () => ({ mutate: assignMutate, isPending: false }),
  useUnassignTag: () => ({ mutate: unassignMutate, isPending: false }),
}));

import { TagChips, TagEditor } from "@/components/access/TagEditor";

const tag = (overrides: Partial<Tag>): Tag => ({
  id: "t1",
  key: "pii",
  name: "PII",
  description: "",
  color: "#e11d48",
  assignment_count: 0,
  created_at: "2026-01-01T00:00:00Z",
  required_roles: [],
  required_groups: [],
  ...overrides,
});

function renderEditor() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TagEditor ws="eng" kind="dataset" id="d1" />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  objectTagsMock.mockReset().mockReturnValue({ data: [], isLoading: false });
  catalogTagsMock.mockReset().mockReturnValue({ data: [], isLoading: false });
  assignMutate.mockReset();
  unassignMutate.mockReset();
});

describe("TagEditor", () => {
  it("renders the object's current tags as removable chips; removing one calls unassign", async () => {
    objectTagsMock.mockReturnValue({ data: [tag({ id: "t1", name: "PII" })], isLoading: false });
    catalogTagsMock.mockReturnValue({
      data: [tag({ id: "t1", name: "PII" }), tag({ id: "t2", name: "Sensitive" })],
      isLoading: false,
    });
    const user = userEvent.setup();
    renderEditor();

    expect(screen.getByText("PII")).toBeInTheDocument();
    await user.click(screen.getByLabelText("Remove PII"));
    expect(unassignMutate).toHaveBeenCalledWith("t1");
  });

  it("the add control lists catalog tags not already assigned; picking one assigns it", async () => {
    objectTagsMock.mockReturnValue({ data: [tag({ id: "t1", name: "PII" })], isLoading: false });
    catalogTagsMock.mockReturnValue({
      data: [tag({ id: "t1", name: "PII" }), tag({ id: "t2", name: "Sensitive" })],
      isLoading: false,
    });
    const user = userEvent.setup();
    renderEditor();

    await user.click(screen.getByLabelText("Add tag"));
    expect(screen.queryByRole("option", { name: "PII" })).toBeNull();
    await user.click(await screen.findByRole("option", { name: "Sensitive" }));

    expect(assignMutate).toHaveBeenCalledWith("t2");
  });

  it("shows a muted hint when the catalog has no tags defined", () => {
    objectTagsMock.mockReturnValue({ data: [], isLoading: false });
    catalogTagsMock.mockReturnValue({ data: [], isLoading: false });
    renderEditor();

    expect(
      screen.getByText(/create them in Control Plane.*Classifications/i),
    ).toBeInTheDocument();
  });

  it("shows a loading indicator while fetching", () => {
    objectTagsMock.mockReturnValue({ data: undefined, isLoading: true });
    catalogTagsMock.mockReturnValue({ data: undefined, isLoading: true });
    renderEditor();

    expect(screen.getByText("…")).toBeInTheDocument();
  });
});

describe("TagChips", () => {
  it("renders a read-only list of tags with no remove control", () => {
    render(<TagChips tags={[tag({ id: "t1", name: "PII" }), tag({ id: "t2", name: "Sensitive" })]} />);

    expect(screen.getByText("PII")).toBeInTheDocument();
    expect(screen.getByText("Sensitive")).toBeInTheDocument();
    expect(screen.queryByLabelText("Remove PII")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("renders nothing meaningful for an empty tag list", () => {
    render(<TagChips tags={[]} />);
    expect(screen.getByText("—")).toBeInTheDocument();
  });
});
