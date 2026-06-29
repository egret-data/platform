import { render, screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import ControlPlaneEngines from "@/routes/control-plane/ControlPlaneEngines";

describe("ControlPlaneEngines frame", () => {
  test("renders the engine type tabs, empty state, and a disabled Add button", () => {
    render(<ControlPlaneEngines />);

    // Title + the three engine-type tabs.
    expect(screen.getByText("Engines")).toBeInTheDocument();
    for (const name of ["Trino", "StarRocks", "Spark Connect"]) {
      expect(screen.getByRole("tab", { name })).toBeInTheDocument();
    }

    // The default (Trino) tab shows the empty state.
    expect(screen.getByText(/No Trino engines yet/i)).toBeInTheDocument();

    // Add is a stub (disabled) — no backend yet.
    const add = screen.getByRole("button", { name: /Add engine/i });
    expect(add).toBeDisabled();
  });
});
