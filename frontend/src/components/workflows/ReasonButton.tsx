import type { ComponentProps, ReactNode } from "react";
import { Button } from "@/components/ui/button";
import type { Enablement } from "@/components/workflows/draftState";

// Lifted out of `routes/workflows/FlowEditPage.tsx` (W7b Task 3) so the
// execution page can use them without importing the flow editor, whose
// module registers the editor tabs as a side effect.

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
 * The reasons of the disabled actions, as visible text under them: "Create version:
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
