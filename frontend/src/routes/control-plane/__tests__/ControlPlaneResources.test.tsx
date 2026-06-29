/**
 * ControlPlaneResources frame tests
 *
 * The Resources page is a no-backend structured frame: type tabs
 * (Trino / StarRocks / Spark Connect), an empty-state table per tab, and a
 * stub (disabled) "Add resource" button. These tests assert the frame shape
 * without any API interaction.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import ControlPlaneResources from "@/routes/control-plane/ControlPlaneResources";

describe("ControlPlaneResources frame", () => {
  test("renders the three resource-type tabs", () => {
    render(<ControlPlaneResources />);
    expect(screen.getByRole("tab", { name: "Trino" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "StarRocks" })).toBeInTheDocument();
    expect(
      screen.getByRole("tab", { name: "Spark Connect" }),
    ).toBeInTheDocument();
  });

  test("shows an empty state for the active (Trino) tab", () => {
    render(<ControlPlaneResources />);
    expect(
      screen.getByText(/No Trino resources yet — connect one in a later phase\./i),
    ).toBeInTheDocument();
  });

  test("the Add resource button is present and disabled", () => {
    render(<ControlPlaneResources />);
    const addButton = screen.getByRole("button", { name: /add resource/i });
    expect(addButton).toBeInTheDocument();
    expect(addButton).toBeDisabled();
  });
});
