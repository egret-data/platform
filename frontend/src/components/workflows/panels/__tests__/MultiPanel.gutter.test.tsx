import type { ComponentProps, ReactNode } from "react";
import { render } from "@testing-library/react";
import { Code, FileText } from "lucide-react";
import { beforeAll, describe, expect, it, vi } from "vitest";

/**
 * The resize gutter's hit area (W7a Task 8). It cannot be measured in jsdom,
 * which lays nothing out, so this pins the value the frame hands the library:
 * `react-resizable-panels` stretches a separator's hit area to
 * `resizeTargetMinimumSize` (10px fine, 20px coarse by default), and with a
 * narrower gutter that reached 3px into the next panel's first tab, where a drag
 * resized the panels instead of moving the tab. The e2e drags from 2px inside
 * that tab; this keeps the value in CI.
 */
const groups = vi.hoisted(() => [] as Record<string, unknown>[]);
const separators = vi.hoisted(() => [] as Record<string, unknown>[]);
vi.mock("react-resizable-panels", () => ({
  Group: ({ children, ...props }: { children: ReactNode } & Record<string, unknown>) => {
    groups.push(props);
    return <div>{children}</div>;
  },
  Panel: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  Separator: (props: Record<string, unknown>) => {
    separators.push(props);
    return <div role="separator" />;
  },
}));

import { MultiPanel } from "@/components/workflows/panels/MultiPanel";
import { CODE_TAB } from "@/components/workflows/panels/layout";
import { registerTab } from "@/components/workflows/panels/registry";

beforeAll(() => {
  registerTab({ id: CODE_TAB, title: "Code", icon: Code, closable: false, render: () => null });
  registerTab({ id: "docs", title: "Docs", icon: FileText, closable: true, render: () => null });
});

describe("MultiPanel: the resize gutter", () => {
  it("hands the library a hit area exactly as wide as the 8px gutter, on fine and coarse pointers", () => {
    render(
      <MultiPanel
        layout={{ panels: [
          { id: "p1", size: 60, tabs: [CODE_TAB], active: CODE_TAB },
          { id: "p2", size: 40, tabs: ["docs"], active: "docs" },
        ] }}
        onChange={() => {}}
        context={{}}
      />,
    );
    expect(groups.at(-1)?.resizeTargetMinimumSize).toEqual({ fine: 8, coarse: 8 });
    // …and the gutter is that wide: `w-2` is 8px.
    expect(String((separators.at(-1) as ComponentProps<"div">).className).split(/\s+/)).toContain("w-2");
  });
});
