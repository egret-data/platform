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
  trinoListError: null,
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

  test("names the engines that were not checked, with the reason", () => {
    respond({
      ...EMPTY,
      enginesNotChecked: [
        { engine: "sr-down", reason: "not running: provisioning" },
        { engine: "sr-ext", reason: "external" },
      ],
    });
    render(<ViewExposureSection tagId="t1" engineGated />);

    const note = screen.getByText(/not checked/i).closest("div");
    expect(note).not.toBeNull();
    const down = within(note as HTMLElement).getByText(/sr-down/);
    expect(down).toHaveTextContent(/not running: provisioning/);
    expect(down).toHaveTextContent(/start the engine/i);
    const ext = within(note as HTMLElement).getByText(/sr-ext/);
    expect(ext).toHaveTextContent(/external/);
    expect(ext).toHaveTextContent(/egret does not connect to engines it does not run/i);
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

  test("lists the Trino views with mode, owner, clearance and reason", () => {
    respond({
      ...EMPTY,
      trinoViews: [
        {
          catalog: "ice",
          schema: "sales",
          name: "v_definer",
          securityMode: "definer",
          owner: "alice",
          ownerCleared: false,
          reason: "classified",
          lineageState: "computed",
          matchedBy: "lineage",
        },
        {
          catalog: "ice",
          schema: "sales",
          name: "v_masked",
          securityMode: "invoker",
          owner: null,
          ownerCleared: null,
          reason: "mask_or_row_filter",
          lineageState: "computed",
          matchedBy: "lineage",
        },
        {
          catalog: "ice",
          schema: "sales",
          name: "v_unknown",
          securityMode: "unknown",
          securityModeError: "load_view: connection reset",
          owner: null,
          ownerCleared: null,
          reason: "classified",
          lineageState: "failed",
          lineageDetail: "engine trino-1: Access Denied",
          matchedBy: "lineage",
        },
      ],
    });
    render(<ViewExposureSection tagId="t1" engineGated />);

    const section = screen.getByRole("region", { name: /trino views/i });
    expect(within(section).getByText(/definer views run as their owner/i)).toBeInTheDocument();
    expect(within(section).getByText(/invoker views run as the reader/i)).toBeInTheDocument();
    const table = within(section).getByRole("table");

    const definer = within(table).getByText("ice.sales.v_definer").closest("tr") as HTMLElement;
    expect(definer).toHaveTextContent(/definer/i);
    expect(definer).toHaveTextContent("alice");
    expect(definer).toHaveTextContent(/not cleared/i);
    expect(definer).toHaveTextContent(/reads classified data/i);

    const masked = within(table).getByText("ice.sales.v_masked").closest("tr") as HTMLElement;
    expect(masked).toHaveTextContent(/invoker/i);
    expect(masked).toHaveTextContent(/mask or row filter/i);

    const unknown = within(table).getByText("ice.sales.v_unknown").closest("tr") as HTMLElement;
    expect(unknown).toHaveTextContent(/unknown/i);
    expect(unknown).toHaveTextContent("connection reset");
    expect(unknown).toHaveTextContent(/lineage failed/i);
    expect(unknown).toHaveTextContent("engine trino-1: Access Denied");
  });

  test("a view with missing lineage says egret has not judged it", () => {
    respond({
      ...EMPTY,
      trinoViews: [
        {
          catalog: "ice",
          schema: "sales",
          name: "v_old",
          securityMode: "definer",
          owner: "bob",
          ownerCleared: null,
          reason: "classified",
          lineageState: "missing",
          matchedBy: "lineage",
        },
      ],
    });
    render(<ViewExposureSection tagId="t1" engineGated />);

    const row = screen.getByText("ice.sales.v_old").closest("tr") as HTMLElement;
    expect(row).toHaveTextContent(/lineage not computed/i);
    expect(row).toHaveTextContent(/clearance unknown/i);
  });

  test("an empty Trino list says none were found", () => {
    respond(EMPTY);
    render(<ViewExposureSection tagId="t1" engineGated />);

    const section = screen.getByRole("region", { name: /trino views/i });
    expect(within(section).getByText(/no trino views/i)).toBeInTheDocument();
    expect(within(section).queryByRole("table")).not.toBeInTheDocument();
  });

  test("a failed Trino listing shows the error and never says there are none", () => {
    respond({
      ...EMPTY,
      trinoViews: null,
      trinoListError: "the views of ice.sales could not be listed: boom",
    });
    render(<ViewExposureSection tagId="t1" engineGated />);

    const section = screen.getByRole("region", { name: /trino views/i });
    expect(within(section).getByRole("alert")).toHaveTextContent(/boom/);
    expect(within(section).queryByText(/no trino views/i)).not.toBeInTheDocument();
  });
});
