import { ChevronRight } from "lucide-react";
import type { ReactNode } from "react";

/**
 * A scoped caveat: **the consequence in the summary, the explanation behind
 * it.** `docs/en/ui-conventions.md` §4.4.
 *
 * Lifted out of `AccessSection.tsx`, where it was written for one row's warning
 * and then wanted by four other places on the same feature (the Service-account
 * picker and the trial warning in `TryIt`, the two caveats around the editor in
 * `Query`, the preview note in `PreviewBlock`). It is exported from the feature
 * rather than promoted to `components/` because the rule it encodes is written
 * down for this editor and has not been measured anywhere else.
 *
 * **What it is for.** The browser pass measured the cost of not having it. At a
 * 786px content box one granted `Access` row carrying 155 characters of caveat
 * was 293px tall against an 89px neighbour — rows disagreeing visually for a
 * reason that was not about importance. The Data API audit then found the same
 * shape at section scale: `Try it` carried ~950 characters of standing prose
 * over one dropdown and one button.
 *
 * **Why this is not the "drawer closed by default" mistake.** `PreviewBlock`
 * records the opposite ruling for its own warning copy: a reader who has to
 * open something to learn a call is real has already pressed the button. That
 * rule holds, and the split is what satisfies it — the load-bearing fact is the
 * SUMMARY and is on screen with nothing clicked. What is behind the disclosure
 * is the *why* and the *where to go instead*, which is what a reader goes
 * looking for once the headline has stopped them. Collapsing the headline would
 * be the mistake; collapsing the paragraph under it is the point.
 *
 * **Native `<details>`/`<summary>`, never `ui/collapsible`** (§11.2). The
 * endpoint shell wraps every section in `<fieldset disabled={trashed}>`, which
 * disables every BUTTON under it — a Radix trigger included — so on a trashed
 * endpoint a Radix collapsible could not be opened and the caveat would be
 * unreachable on exactly the endpoint where an admin is deciding whether to
 * restore it. A `<summary>` is not a form control and the fieldset does not
 * touch it. It is also not a hover affordance: `<summary>` is focusable and
 * toggles on Enter/Space, so keyboard and touch reach it the way a pointer
 * does, where a `title` tooltip would have hidden it from both.
 *
 * **Collapse a caveat because it is big, not because its neighbour is.** Two
 * lines with no explanation to hold cost a click to reveal nothing —
 * `Access`'s "Checking workspace-inherited invoke…" and `TryIt`'s
 * published-version note both stay plain text for that reason.
 */
export function Caveat({
  summary,
  className,
  children,
}: {
  /** The consequence. On screen with nothing clicked, so keep it to a line. */
  summary: ReactNode;
  /** Type scale for the call site — the cell or panel decides, not this. */
  className?: string;
  children: ReactNode;
}) {
  return (
    <details className={className ? `group ${className}` : "group"}>
      {/* `list-none` plus an explicit chevron rather than the UA marker, which
          renders at a different size and offset per browser and cannot be
          aligned with what sits above it. Same idiom as `Query.tsx`'s strip. */}
      <summary className="flex cursor-pointer list-none items-start gap-1 text-muted-foreground hover:text-foreground">
        <ChevronRight
          className="mt-px h-3 w-3 shrink-0 transition-transform group-open:rotate-90"
          aria-hidden="true"
        />
        <span className="underline decoration-dotted underline-offset-2">{summary}</span>
      </summary>
      <div className="mt-1 pl-4 text-muted-foreground">{children}</div>
    </details>
  );
}
