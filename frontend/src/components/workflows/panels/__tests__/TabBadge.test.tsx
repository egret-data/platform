import { render, screen } from "@testing-library/react";
import { Code, TriangleAlert } from "lucide-react";
import { describe, expect, it } from "vitest";
import { MultiPanel } from "@/components/workflows/panels/MultiPanel";
import { CODE_TAB, type Layout } from "@/components/workflows/panels/layout";
import { registerTab } from "@/components/workflows/panels/registry";

type Ctx = { errors: number };

registerTab<Ctx>({ id: CODE_TAB, title: "Code", icon: Code, closable: false, render: () => <p>code</p> });
registerTab<Ctx>({
  id: "problems",
  title: "Problems",
  icon: TriangleAlert,
  closable: true,
  render: () => <p>problems</p>,
  badge: (ctx) => (ctx.errors > 0 ? <span>{`, ${ctx.errors} errors`}</span> : null),
});

const LAYOUT: Layout = {
  panels: [{ id: "p1", size: 100, tabs: [CODE_TAB, "problems"], active: CODE_TAB }],
};

describe("a tab's badge", () => {
  it("is drawn in the tab from the frame's context, and follows it", () => {
    const { rerender } = render(<MultiPanel<Ctx> layout={LAYOUT} onChange={() => {}} context={{ errors: 3 }} />);
    expect(screen.getByRole("tab", { name: "Problems, 3 errors" })).toBeInTheDocument();
    // A tab without one is unchanged.
    expect(screen.getByRole("tab", { name: "Code" })).toBeInTheDocument();

    rerender(<MultiPanel<Ctx> layout={LAYOUT} onChange={() => {}} context={{ errors: 0 }} />);
    expect(screen.getByRole("tab", { name: "Problems" })).toBeInTheDocument();
    // The actions menu names the tab by its title alone.
    expect(screen.getByRole("button", { name: "Tab actions: Code" })).toBeInTheDocument();
  });
});
