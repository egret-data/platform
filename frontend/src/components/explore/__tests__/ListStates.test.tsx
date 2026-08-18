import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ListStates } from "@/components/explore/ListStates";

const empty = <p>No endpoints yet</p>;

describe("ListStates", () => {
  it("shows a skeleton while loading and nothing else", () => {
    // 7, not the default 4 and not the 5 every other case here passes: a column
    // count that no default could produce is what makes the last assertion
    // prove `skeletonCols` was actually threaded through.
    render(
      <ListStates isLoading isError={false} errorText="Could not load." hasData={false} skeletonCols={7} empty={empty}>
        <p>rows</p>
      </ListStates>,
    );
    // Two absences alone were vacuous — `if (isLoading) return null;` satisfied
    // them, and nothing anywhere asserted `skeletonCols` reached anything.
    const skeleton = screen.getByTestId("list-skeleton");
    expect(skeleton).toBeInTheDocument();
    expect(skeleton.firstElementChild?.childElementCount).toBe(7);

    expect(screen.queryByText("rows")).not.toBeInTheDocument();
    expect(screen.queryByText("No endpoints yet")).not.toBeInTheDocument();
  });

  it("shows the ERROR, not the empty state, when the query failed", () => {
    render(
      <ListStates isLoading={false} isError error={new Error("boom")} errorText="Could not load endpoints." hasData={false} skeletonCols={5} empty={empty}>
        <p>rows</p>
      </ListStates>,
    );
    // This is the whole reason the component exists. Folding `isError` into
    // "empty" tells an operator their workspace has nothing when in fact the
    // request failed — the exact defect two account pickers shipped, which sent
    // an operator to create a service account that already existed.
    expect(screen.getByText("boom")).toBeInTheDocument();
    expect(screen.queryByText("No endpoints yet")).not.toBeInTheDocument();
  });

  it("falls back to errorText when the error carries no message", () => {
    render(
      <ListStates isLoading={false} isError error={new Error("")} errorText="Could not load endpoints." hasData={false} skeletonCols={5} empty={empty}>
        <p>rows</p>
      </ListStates>,
    );
    // `api/client.ts` throws a message-less `UnauthorizedError` on 401, and
    // `ApiError.from` falls back to `res.statusText`, which is "" over HTTP/2.
    // Without this fallback the alert renders blank — a failure that looks
    // exactly like a dead page.
    expect(screen.getByText("Could not load endpoints.")).toBeInTheDocument();
  });

  it("shows the empty state when the query succeeded with no rows", () => {
    render(
      <ListStates isLoading={false} isError={false} errorText="Could not load." hasData={false} skeletonCols={5} empty={empty}>
        <p>rows</p>
      </ListStates>,
    );
    expect(screen.getByText("No endpoints yet")).toBeInTheDocument();
    expect(screen.queryByText("rows")).not.toBeInTheDocument();
  });

  it("renders children when there is data", () => {
    render(
      <ListStates isLoading={false} isError={false} errorText="Could not load." hasData skeletonCols={5} empty={empty}>
        <p>rows</p>
      </ListStates>,
    );
    expect(screen.getByText("rows")).toBeInTheDocument();
  });
});
