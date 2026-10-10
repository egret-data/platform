import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Tag } from "@/api/tags";
import type { ResourceTagAssignment } from "@/api/catalogTags";
import { ApiError } from "@/api/client";

const catalogTagsMock = vi.fn();
const resourceTagsMock = vi.fn();
const assignMutate = vi.fn();
const unassignMutate = vi.fn();
/** The assign mutation's react-query state, so a test can put it in the FAILED
 *  state the backend's 400 produces. `useAssignResourceTag`'s own job — turning
 *  that 400 into this state — is measured with the real hook and a real
 *  `apiJson` in `src/api/__tests__/catalogTags.test.tsx`; this file measures what
 *  the component DOES with it. */
let assignState: { isError: boolean; error: Error | null } = { isError: false, error: null };
/** Same, for the REMOVE half (SP4d debt Task 11). `useUnassignResourceTag`'s own job
 *  — letting the `ApiError` out of its `mutationFn` so react-query exposes it here —
 *  is measured with the real hook over a stubbed `fetch` in
 *  `src/api/__tests__/catalogTags.test.tsx`; this file measures what the component
 *  DOES with it. The split is not caution: a `mutationFn` that swallowed the
 *  rejection would kill that api test and leave every assertion in THIS file green,
 *  because this file replaces the hook. */
let unassignState: { isError: boolean; error: Error | null } = { isError: false, error: null };

vi.mock("@/api/tags", () => ({
  useTags: () => catalogTagsMock(),
}));

vi.mock("@/api/catalogTags", () => ({
  useResourceTags: (...args: unknown[]) => resourceTagsMock(...args),
  useAssignResourceTag: () => ({ mutate: assignMutate, isPending: false, ...assignState }),
  useUnassignResourceTag: () => ({ mutate: unassignMutate, isPending: false, ...unassignState }),
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
  required_roles: [],
  required_groups: [],
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
  assignState = { isError: false, error: null };
  unassignState = { isError: false, error: null };
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

  /**
   * **SP4d debt Task 3, fix round, Minor 1 — the ruling's justification, made true
   * where a steward can see it.**
   *
   * Migration 0177 makes some coordinates unstorable and `assign_catalog` refuses
   * them with a 400 that NAMES the offending level. The trade that justified
   * refusing instead of storing-and-declining was "a loud 400 at write time beats a
   * silent decline at read time" — and as first shipped this component had no
   * `onError` and threw the mutation result away, so the only visible effect of a
   * refusal was the trigger snapping back to its placeholder. The tag silently was
   * not applied: the exact failure mode the refusal was supposed to trade away.
   *
   * The assertion is the backend's OWN SENTENCE, not a UI-authored one — the level
   * name is the actionable half, and a frontend copy of it would be a fifth place
   * for the predicate's wording to drift. Both arms are asserted, because an alert
   * that renders unconditionally would pass the first half alone.
   */
  it("renders the backend's refusal, naming the level, when a coordinate cannot be stored", () => {
    resourceTagsMock.mockReturnValue({ data: [], isLoading: false });
    catalogTagsMock.mockReturnValue({ data: [tag({ id: "t1", name: "PII" })], isLoading: false });

    // The happy path says nothing — so the assertion below can fail.
    renderEditor({ column: "ord*ers" }).unmount();
    expect(screen.queryByTestId("catalog-tag-assign-error")).not.toBeInTheDocument();

    assignState = {
      isError: true,
      error: new Error("column must not contain `*` or `?`: the engine would match it as a wildcard and egret could not resolve the tag"),
    };
    renderEditor({ column: "ord*ers" });

    const alert = screen.getByTestId("catalog-tag-assign-error");
    expect(alert).toHaveTextContent(/could not add that tag/i);
    expect(alert).toHaveTextContent(/column must not contain/i);
    expect(alert).toHaveTextContent(/wildcard/i);
    // Announced, not merely present: a steward who just picked from a Select is
    // looking at the Select, not below it.
    expect(alert).toHaveAttribute("role", "alert");
  });

  /**
   * **SP4d debt Task 11 — the REMOVE half of Task 3's fix.**
   *
   * Before this, `unassign.mutate()`'s result was discarded and the component had no
   * `onError`, so a refused removal left the chip exactly where it was (the query is
   * invalidated by `onSuccess` only) and said nothing. A steward could not tell a
   * refusal from a slow refetch.
   *
   * BOTH arms are asserted, and they are different arms on purpose. The 403 is the
   * routinely reachable failure — the route is gated `catalog_registry:read` while
   * `unassign_resource_tag` requires `catalog_registry:update`, and the Remove button
   * carries no permission check — and its `ApiError.message` is `Error::Forbidden`'s
   * Display, the bare word "forbidden", so the alert names the permission instead
   * (what `BrowseTab` already does for the read 403 on this same route). Every other
   * status shows the backend's message verbatim, which is what a 500 or a future
   * refusal with real wording needs.
   *
   * Note what this test does NOT assert: migration 0177's wildcard 400.
   * `unassign_catalog` deliberately has no `unresolvable_coordinate` check, so no
   * coordinate can make THIS mutation fail — asserting that shape would pin a
   * fiction.
   */
  it("renders a refusal to remove a tag — naming the permission on 403, verbatim otherwise", () => {
    resourceTagsMock.mockReturnValue({
      data: [assignment({ column: "", tag: tag({ id: "t1", name: "PII" }) })],
      isLoading: false,
    });
    catalogTagsMock.mockReturnValue({ data: [tag({ id: "t1", name: "PII" })], isLoading: false });

    // The happy path says nothing — so the assertions below can fail.
    renderEditor().unmount();
    expect(screen.queryByTestId("catalog-tag-unassign-error")).not.toBeInTheDocument();

    // Arm 1: the reachable 403. "forbidden" must NOT be all a steward is told.
    unassignState = { isError: true, error: new ApiError(403, "forbidden") };
    const forbidden = renderEditor();
    const alert403 = screen.getByTestId("catalog-tag-unassign-error");
    expect(alert403).toHaveTextContent(/could not remove that tag/i);
    expect(alert403).toHaveTextContent("catalog_registry:update");
    expect(alert403).not.toHaveTextContent(/: forbidden/);
    // Announced, not merely present: the steward is looking at the chip they clicked.
    expect(alert403).toHaveAttribute("role", "alert");
    forbidden.unmount();

    // Arm 2: anything else shows the backend's own sentence, unparaphrased.
    unassignState = {
      isError: true,
      error: new ApiError(500, "recompiling the bound policy profiles failed"),
    };
    renderEditor();
    const alert500 = screen.getByTestId("catalog-tag-unassign-error");
    expect(alert500).toHaveTextContent("recompiling the bound policy profiles failed");
    expect(alert500).not.toHaveTextContent("catalog_registry:update");
  });

  it("shows a muted hint when the catalog has no tags defined", () => {
    resourceTagsMock.mockReturnValue({ data: [], isLoading: false });
    catalogTagsMock.mockReturnValue({ data: [], isLoading: false });
    renderEditor();

    expect(
      screen.getByText(/create them in Governance.*Classifications/i),
    ).toBeInTheDocument();
  });
});
