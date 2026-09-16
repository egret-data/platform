import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, test } from "vitest";

import { ApiError } from "@/api/client";
import type { EngineDependentDto } from "@/api/generated/model/engineDependentDto";
import {
  EngineDependentsTable,
  dependentsFromError,
} from "@/routes/control-plane/EngineDependentsTable";

const DEPENDENTS: EngineDependentDto[] = [
  {
    workspaceId: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
    workspaceName: "Engineering",
    workspaceSlug: "eng",
    kind: "dataapi_endpoint",
    count: 25,
    sample: ["a", "b", "c", "d", "e"],
  },
  {
    workspaceId: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
    workspaceName: "Engineering",
    workspaceSlug: "eng",
    kind: "dataapi_endpoint_trashed",
    count: 1,
    sample: ["old-orders"],
  },
  {
    workspaceId: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
    workspaceName: "Data",
    workspaceSlug: "data",
    kind: "schedule",
    count: 2,
    sample: ["nightly", "weekly"],
  },
];

describe("dependentsFromError", () => {
  test("reads the list from an engine_has_dependents 409", () => {
    const e = new ApiError(409, "still in use", "engine_has_dependents", {
      error: "still in use",
      code: "engine_has_dependents",
      dependents: DEPENDENTS,
    });
    expect(dependentsFromError(e)).toEqual(DEPENDENTS);
  });

  test("is null for any other error", () => {
    expect(dependentsFromError(new ApiError(409, "c", "constraint_violation", { constraint: "x" }))).toBeNull();
    expect(dependentsFromError(new ApiError(502, "boom", "engine_cluster_delete_failed"))).toBeNull();
    expect(dependentsFromError(new Error("network"))).toBeNull();
  });

  test("is null for an empty dependents array (an empty list must not open a blocked dialog)", () => {
    const e = new ApiError(409, "still in use", "engine_has_dependents", {
      error: "still in use",
      code: "engine_has_dependents",
      dependents: [],
    });
    expect(dependentsFromError(e)).toBeNull();
  });
});

describe("EngineDependentsTable", () => {
  test("one row per workspace and kind, with counts, examples and a link", () => {
    render(
      <MemoryRouter>
        <EngineDependentsTable dependents={DEPENDENTS} />
      </MemoryRouter>,
    );
    const rows = within(screen.getByRole("table")).getAllByRole("row");
    expect(rows).toHaveLength(4); // header + 3

    expect(within(rows[1]).getByText("Data API endpoints")).toBeInTheDocument();
    expect(within(rows[1]).getByText("25")).toBeInTheDocument();
    // 25 > 5 names shown → the list says it is truncated.
    expect(within(rows[1]).getByText("a, b, c, d, e, …")).toBeInTheDocument();
    expect(within(rows[1]).getByRole("link", { name: "Open" })).toHaveAttribute(
      "href",
      "/explore/eng/data-api/endpoints",
    );

    expect(within(rows[2]).getByText("Data API endpoints in trash")).toBeInTheDocument();
    expect(within(rows[3]).getByText("Alerts & reports")).toBeInTheDocument();
    expect(within(rows[3]).getByText("nightly, weekly")).toBeInTheDocument();
    expect(within(rows[3]).getByRole("link", { name: "Open" })).toHaveAttribute(
      "href",
      "/explore/data/alerts",
    );
  });
});
