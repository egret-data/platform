import { useCallback, useEffect, useRef, type ComponentProps, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import type { Enablement } from "@/components/workflows/draftState";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { MultiPanel } from "@/components/workflows/panels/MultiPanel";
import { activateTab, CODE_TAB, DEFAULT_FLOW_LAYOUT, type Layout } from "@/components/workflows/panels/layout";
import { LAYOUT_KEY_PREFIX, usePersistedLayout } from "@/components/workflows/panels/usePersistedLayout";
// Registers the Code, Docs and Problems tabs with the panel frame (spec §5.1).
// Here, in the flow pages' own lazy chunk, so the editor and `yaml` load with
// the editor and not with the flow list (`scripts/assert-workflows-lazy.mjs`).
import "@/components/workflows/editor/registerEditorTabs";

export type FlowLayout = {
  layout: Layout;
  setLayout: (next: Layout) => void;
  /** Select the Code tab in its panel; `useFlowEditor` calls it when a tab asks for the editor. */
  revealCode: () => void;
};

/**
 * The editor layout of one flow, kept per viewer (`usePersistedLayout`). An
 * empty `flowKey` (the create page) keeps it in memory until the flow exists;
 * {@link storeFlowLayout} then files it under the new flow's id.
 *
 * It lives in the page rather than in the Edit tab because `useFlowEditor`,
 * which the page calls, needs `revealCode` before the tab renders.
 */
export function useFlowLayout(flowKey: string): FlowLayout {
  const [layout, setLayout] = usePersistedLayout(flowKey, DEFAULT_FLOW_LAYOUT);
  const latest = useRef(layout);
  useEffect(() => { latest.current = layout; });
  const revealCode = useCallback(() => {
    const next = activateTab(latest.current, CODE_TAB);
    if (next !== latest.current) setLayout(next);
  }, [setLayout]);
  return { layout, setLayout, revealCode };
}

/**
 * File `layout` under a flow that was just created, so the editor it opens in
 * keeps the panels the create page had (spec §5.4). A preference write
 * (FE-DATA-3): without storage the new flow opens with the default layout.
 */
export function storeFlowLayout(flowId: string, layout: Layout): void {
  try {
    localStorage.setItem(`${LAYOUT_KEY_PREFIX}${flowId}`, JSON.stringify(layout));
  } catch {
    // A preference only.
  }
}

/**
 * The flow detail's Edit tab, and the body of the create page: the
 * multi-panel editor (spec §5.1) over the page's {@link EditorContext}. The
 * page owns the context and the layout, so switching to another detail tab
 * and back loses neither.
 */
export function FlowEditPage({ ctx, flowLayout }: { ctx: EditorContext; flowLayout: FlowLayout }) {
  return (
    <div className="h-full min-h-0 border-t border-border">
      <MultiPanel<EditorContext> layout={flowLayout.layout} onChange={flowLayout.setLayout} context={ctx} />
    </div>
  );
}

/**
 * `Ctrl/Cmd+S` saves anywhere on a flow page (spec §5.2), not only inside
 * Monaco, and never opens the browser's "Save page". The Code tab also binds
 * it inside Monaco; `save` must therefore ignore a second call while one is
 * in flight, which both pages' saves do.
 */
export function useSaveShortcut(save: () => void, enabled: boolean): void {
  const latest = useRef(save);
  useEffect(() => { latest.current = save; });
  useEffect(() => {
    if (!enabled) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        latest.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [enabled]);
}

/**
 * A header action that says why it is disabled (FE-ACTION-4). The reason is
 * shown as text beside the actions by {@link DisabledReasons}, which keyboard
 * and touch users see too (a disabled button cannot take focus, so a tooltip
 * alone would reach only a mouse); `reasonId` ties that text to the button as
 * its accessible description. `title` adds the same sentence on hover.
 */
export function ReasonButton({
  enablement,
  reasonId,
  onClick,
  children,
  variant,
}: {
  enablement: Enablement;
  reasonId: string;
  onClick: () => void;
  children: ReactNode;
  variant?: ComponentProps<typeof Button>["variant"];
}) {
  return (
    <Button
      type="button"
      size="sm"
      variant={variant}
      disabled={!enablement.ok}
      title={enablement.ok ? undefined : enablement.reason}
      aria-describedby={enablement.ok ? undefined : reasonId}
      onClick={onClick}
    >
      {children}
    </Button>
  );
}

/**
 * The reasons of the disabled actions, as visible text under them: "Publish:
 * Save first". Each reason's element has the id its button names in
 * `aria-describedby`; the action's name is `aria-hidden` there, so the
 * button's description is the reason alone.
 */
export function DisabledReasons({ actions }: { actions: { id: string; label: string; enablement: Enablement }[] }) {
  const off = actions.filter((a) => !a.enablement.ok);
  if (off.length === 0) return null;
  return (
    <p className="flex flex-wrap justify-end gap-x-3 text-xs text-muted-foreground">
      {off.map((a) => (
        <span key={a.id} id={a.id}>
          <span aria-hidden>{a.label}: </span>
          {a.enablement.ok ? null : a.enablement.reason}
        </span>
      ))}
    </p>
  );
}
