import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { useControlPlaneTagsGetViewExposure } from "@/api/generated/control-plane-tags/control-plane-tags";
import type { ViewExposureDto } from "@/api/generated/model/viewExposureDto";
import ViewExposureSection from "@/routes/governance/ViewExposureSection";

vi.mock("@/api/generated/control-plane-tags/control-plane-tags");

type Hook = ReturnType<typeof useControlPlaneTagsGetViewExposure>;

function respond(body: ViewExposureDto) {
  vi.mocked(useControlPlaneTagsGetViewExposure).mockReturnValue({
    data: { status: 200, data: body, headers: new Headers() },
    isLoading: false,
    isError: false,
    error: null,
  } as unknown as Hook);
}

const WARNING = /may hold real classified values/i;

describe("ViewExposureSection", () => {
  beforeEach(() => vi.clearAllMocks());

  test("lists the materialized views and carries the warning", () => {
    respond({
      starrocksMaterializedViews: [
        { engine: "sr-1", catalog: "lake", database: "sales", name: "mv_copy_of_ssn" },
        { engine: "sr-1", catalog: "lake", database: "sales", name: "mv_daily" },
      ],
      starrocksListError: null,
      trinoViews: [],
    });
    render(<ViewExposureSection tagId="t1" />);

    expect(screen.getByText(WARNING)).toBeInTheDocument();
    expect(screen.getByText(/does not drop them/i)).toBeInTheDocument();
    const table = screen.getByRole("table");
    expect(within(table).getByText("mv_copy_of_ssn")).toBeInTheDocument();
    expect(within(table).getByText("mv_daily")).toBeInTheDocument();
    expect(within(table).getAllByText("sr-1")).toHaveLength(2);
    expect(within(table).getAllByText("lake.sales")).toHaveLength(2);
  });

  test("an empty list says none were found", () => {
    respond({ starrocksMaterializedViews: [], starrocksListError: null, trinoViews: [] });
    render(<ViewExposureSection tagId="t1" />);

    expect(screen.getByText(/no materialized views/i)).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  test("a failed listing shows the error and never says there are none", () => {
    respond({
      starrocksMaterializedViews: null,
      starrocksListError:
        "could not list materialized views on engine sr-1 in lake.sales: connect timed out",
      trinoViews: [],
    });
    render(<ViewExposureSection tagId="t1" />);

    expect(screen.getByRole("alert")).toHaveTextContent(/engine sr-1 in lake\.sales/);
    expect(screen.queryByText(/no materialized views/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    // The warning stays: the views are unknown, not absent.
    expect(screen.getByText(WARNING)).toBeInTheDocument();
  });

  test("a request that fails is an error, not an empty list", () => {
    vi.mocked(useControlPlaneTagsGetViewExposure).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new Error("forbidden"),
    } as unknown as Hook);
    render(<ViewExposureSection tagId="t1" />);

    expect(screen.getByRole("alert")).toHaveTextContent(/forbidden/);
    expect(screen.queryByText(/no materialized views/i)).not.toBeInTheDocument();
  });

  test("asks for the classification it was given", () => {
    respond({ starrocksMaterializedViews: [], starrocksListError: null, trinoViews: [] });
    render(<ViewExposureSection tagId="t-42" />);

    expect(vi.mocked(useControlPlaneTagsGetViewExposure)).toHaveBeenCalledWith(
      "t-42",
      expect.anything(),
    );
  });
});
