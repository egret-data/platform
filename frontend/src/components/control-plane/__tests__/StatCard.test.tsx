import { render, screen } from "@testing-library/react";
import { Server } from "lucide-react";
import { describe, expect, it } from "vitest";

import { StatCard, StatCardRow } from "../StatCard";

describe("StatCard", () => {
  it("renders label, value, hint and an icon", () => {
    render(<StatCard label="Engines" value={7} hint="Registered clusters" icon={Server} />);
    expect(screen.getByText("Engines")).toBeInTheDocument();
    expect(screen.getByText("7")).toBeInTheDocument();
    expect(screen.getByText("Registered clusters")).toBeInTheDocument();
  });

  it("applies the tone colour class to the value", () => {
    render(<StatCard label="Running" value={3} tone="success" />);
    expect(screen.getByText("3")).toHaveClass("text-primary");
  });

  it("StatCardRow lays out its children", () => {
    render(
      <StatCardRow>
        <StatCard label="A" value={1} />
        <StatCard label="B" value={2} />
      </StatCardRow>,
    );
    expect(screen.getByText("A")).toBeInTheDocument();
    expect(screen.getByText("B")).toBeInTheDocument();
  });
});
