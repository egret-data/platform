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

const EMPTY: ViewExposureDto = {
  starrocksMaterializedViews: [],
  starrocksListError: null,
  enginesNotChecked: [],
  trinoViews: [],
};

const WARNING = /may hold real classified values/i;
const COST = /cannot create any materialized view/i;

describe("ViewExposureSection", () => {
  beforeEach(() => vi.clearAllMocks());

  test("lists the materialized views and carries the warning", () => {
    respond({
      ...EMPTY,
      starrocksMaterializedViews: [
        {
          engine: "sr-1",
          catalog: "default_catalog",
          database: "sales",
          name: "mv_copy_of_ssn",
          matchedBy: "baseTable",
        },
        {
          engine: "sr-1",
          catalog: "default_catalog",
          database: "sales",
          name: "mv_daily",
          matchedBy: "baseTable",
        },
      ],
    });
    render(<ViewExposureSection tagId="t1" engineGated />);

    expect(screen.getByText(WARNING)).toBeInTheDocument();
    expect(screen.getByText(/does not drop them/i)).toBeInTheDocument();
    const table = screen.getByRole("table");
    expect(within(table).getByText("mv_copy_of_ssn")).toBeInTheDocument();
    expect(within(table).getByText("mv_daily")).toBeInTheDocument();
    expect(within(table).getAllByText("sr-1")).toHaveLength(2);
    expect(within(table).getAllByText("default_catalog.sales")).toHaveLength(2);
    expect(within(table).queryByText(/matched by text/i)).not.toBeInTheDocument();
  });

  test("a view matched by its text says so", () => {
    respond({
      ...EMPTY,
      starrocksMaterializedViews: [
        {
          engine: "sr-1",
          catalog: "default_catalog",
          database: "db1",
          name: "mv_never_refreshed",
          matchedBy: "text",
        },
      ],
    });
    render(<ViewExposureSection tagId="t1" engineGated />);

    const row = screen.getByText("mv_never_refreshed").closest("tr");
    expect(row).not.toBeNull();
    expect(within(row as HTMLElement).getByText(/matched by text/i)).toBeInTheDocument();
  });

  test("names the engines that were not checked, with their status", () => {
    respond({
      ...EMPTY,
      enginesNotChecked: [{ engine: "sr-down", status: "provisioning" }],
    });
    render(<ViewExposureSection tagId="t1" engineGated />);

    const note = screen.getByText(/not checked/i).closest("div");
    expect(note).not.toBeNull();
    expect(within(note as HTMLElement).getByText(/sr-down/)).toBeInTheDocument();
    expect(within(note as HTMLElement).getByText(/provisioning/)).toBeInTheDocument();
  });

  test("a role clearance names the cost of the engine-wide deny", () => {
    respond(EMPTY);
    render(<ViewExposureSection tagId="t1" engineGated />);

    expect(screen.getByText(COST)).toBeInTheDocument();
    expect(screen.getByText(/not cleared for every classification/i)).toBeInTheDocument();
  });

  test("without a role clearance it claims no deny", () => {
    respond(EMPTY);
    render(<ViewExposureSection tagId="t1" engineGated={false} />);

    expect(screen.getByText(WARNING)).toBeInTheDocument();
    expect(screen.queryByText(COST)).not.toBeInTheDocument();
    expect(screen.queryByText(/denying new materialized views/i)).not.toBeInTheDocument();
  });

  test("an empty list says none were found", () => {
    respond(EMPTY);
    render(<ViewExposureSection tagId="t1" engineGated />);

    expect(screen.getByText(/no materialized views/i)).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  test("a failed listing shows the error and never says there are none", () => {
    respond({
      ...EMPTY,
      starrocksMaterializedViews: null,
      starrocksListError: "could not list materialized views on engine sr-1: connect timed out",
    });
    render(<ViewExposureSection tagId="t1" engineGated />);

    expect(screen.getByRole("alert")).toHaveTextContent(/engine sr-1/);
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
    render(<ViewExposureSection tagId="t1" engineGated />);

    expect(screen.getByRole("alert")).toHaveTextContent(/forbidden/);
    expect(screen.queryByText(/no materialized views/i)).not.toBeInTheDocument();
  });

  test("asks for the classification it was given", () => {
    respond(EMPTY);
    render(<ViewExposureSection tagId="t-42" engineGated />);

    expect(vi.mocked(useControlPlaneTagsGetViewExposure)).toHaveBeenCalledWith(
      "t-42",
      expect.anything(),
    );
  });
});
