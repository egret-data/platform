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
registerTab<Ctx>({
  id: "labelled",
  title: "Labelled",
  icon: TriangleAlert,
  closable: true,
  render: () => <p>labelled</p>,
  badge: (ctx) => (ctx.errors > 0 ? <span className="sr-only">{ctx.errors}</span> : null),
  badgeLabel: (ctx) => (ctx.errors > 0 ? `${ctx.errors} errors` : undefined),
});

const LAYOUT: Layout = {
  panels: [{ id: "p1", size: 100, tabs: [CODE_TAB, "problems", "labelled"], active: CODE_TAB }],
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
    expect(screen.getByRole("tab", { name: "Labelled" })).not.toHaveAttribute("aria-label");
    expect(screen.getByRole("button", { name: "Tab actions: Code" })).toBeInTheDocument();
  });

  it("is named exactly '<title>, <label>' by an aria-label when the badge has words (Chromium spaced a hidden count)", () => {
    render(<MultiPanel<Ctx> layout={LAYOUT} onChange={() => {}} context={{ errors: 3 }} />);
    expect(screen.getByRole("tab", { name: "Labelled, 3 errors" })).toHaveAttribute("aria-label", "Labelled, 3 errors");
  });
});
