import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { DashboardFilterBar } from "../DashboardFilterBar";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "category", operator: "is" },
  { id: "p2", name: "User", slug: "user", type: "id", operator: "is", required: true },
];

describe("DashboardFilterBar", () => {
  it("renders one pill per parameter", () => {
    render(<DashboardFilterBar parameters={params} values={{}} onChange={vi.fn()} optionsBySlug={{}} />);
    expect(screen.getByRole("button", { name: /^Region/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^User/ })).toBeTruthy();
  });
  it("shows a required notice when a required param is blank", () => {
    render(<DashboardFilterBar parameters={params} values={{}} onChange={vi.fn()} optionsBySlug={{}} />);
    expect(screen.getByText(/required filter/i).textContent).toContain("User");
  });
  it("no notice when required params are set", () => {
    render(<DashboardFilterBar parameters={params} values={{ user: "7" }} onChange={vi.fn()} optionsBySlug={{}} />);
    expect(screen.queryByText(/required filter/i)).toBeNull();
  });
});
