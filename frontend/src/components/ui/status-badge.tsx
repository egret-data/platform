import type { ComponentProps, ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/**
 * THE status badge (FE-VISUAL-8, KI-222): a light tint of a state token, ink in
 * `--foreground`, the word in sentence case.
 *
 * Before it the same state was painted three ways — a solid green pill in
 * Control Plane (*active*), a tint in Explore (*Active*), a dot and plain text
 * in settings — and read in two cases.
 *
 * **The tint is the token; the ink is `--foreground` on every tone.** Coloured
 * ink on a wash of its own hue fails 4.5:1 for this 12px text in light mode;
 * `routes/explore/dataapi/chipTone.ts` keeps the measurements, and
 * `tests/e2e/dataapi-badge-ink.spec.ts` measures these values in Chromium.
 *
 * The word carries the state, never the colour alone (FE-A11Y-4); `icon` adds a
 * glyph where a set has more states than a colour can carry.
 *
 * A type or a count is not a status: those stay a plain `Badge`.
 */
export const STATUS_TONE = {
  /** Healthy, serving, current, allowed, active. */
  success: "border-transparent bg-success/15 text-foreground",
  /** Stopped, expired, failing, denied. */
  destructive: "border-transparent bg-destructive/15 text-foreground",
  /** Behind, stale, paused, worth a look — not broken. */
  highlight: "border-transparent bg-highlight/15 text-foreground",
  /** In progress. */
  primary: "border-transparent bg-primary/15 text-foreground",
  /** No state colour: nothing to report, not started yet, a draft. */
  neutral: "border-transparent bg-muted text-foreground",
  /** Finished or switched off without an outcome: archived, disabled, cancelled. */
  inactive: "border-transparent bg-muted-foreground/15 text-foreground",
} as const;

export type StatusTone = keyof typeof STATUS_TONE;

/** `active` → `Active`; `On grace` stays. Server words arrive in lower case. */
export function sentenceCase(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1);
}

export function StatusBadge({
  tone,
  icon: Icon,
  children,
  className,
  ...rest
}: Omit<ComponentProps<"span">, "children"> & {
  tone: StatusTone;
  icon?: LucideIcon;
  /** A string is shown in sentence case; any other node is shown as given. */
  children: ReactNode;
}) {
  return (
    <Badge
      variant="outline"
      data-status-tone={tone}
      className={cn(STATUS_TONE[tone], className)}
      {...rest}
    >
      {Icon ? <Icon aria-hidden="true" /> : null}
      {typeof children === "string" ? sentenceCase(children) : children}
    </Badge>
  );
}
