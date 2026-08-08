import * as React from "react"
import * as CheckboxPrimitive from "@radix-ui/react-checkbox"
import { Check, Minus } from "lucide-react"

import { cn } from "@/lib/utils"

const Checkbox = React.forwardRef<
    React.ElementRef<typeof CheckboxPrimitive.Root>,
    React.ComponentPropsWithoutRef<typeof CheckboxPrimitive.Root>
>(({ className, ...props }, ref) => (
    /* SHAPE IS HOW A USER TELLS "CHOOSE ONE" FROM "CHOOSE MANY", AND THIS ONE WAS ROUND.
     *
     * `rounded-sm` resolves, in this codebase, to `calc(var(--radius) - 4px)` over a
     * `--radius` of 0.625rem — 6px. On a 16px box that is 6 of a possible 8: measured in
     * the browser as `16×16, border-radius: 6px`, three eighths of the side, which leaves
     * almost no straight edge. Beside a `RadioGroupItem` (`rounded-full`, 8px) the two were
     * all but the same control, and egret uses both — a checkbox that reads as a radio
     * tells the reader the choice is exclusive when it is not.
     *
     * REJECTED: changing `--radius-sm`. Tabs, dropdown-menu items, select items, filter
     * pills and a dozen call sites read it, and 6px is correct for a 32px pill. The number
     * was never the bug; handing a fixed radius to a control an eighth of the size was. 4px
     * is a quarter of the side — visibly a box — and is what shadcn's own checkbox ships.
     *
     * THE 24px HIT AREA is a transparent `::before`, not a bigger box. WCAG 2.2 SC 2.5.8
     * (AA) asks for 24×24 CSS px; the painted box has to stay 16px because it is aligned
     * against 20px text lines throughout the app and growing it would reflow every list.
     * A pseudo-element costs no layout and paints nothing, and 4px of bleed is small enough
     * that the densest list in the app — the permission picker's `gap-y-1.5` grid, whose
     * boxes sit 10px apart — still leaves 2px between neighbouring targets, so no click can
     * land on the wrong row. #441 fixed this per-list, by making the chart panel's rows
     * 32px `<label>`s; that is still the better answer where a row CAN be the target, since
     * it makes the text clickable too. This covers the lists where it is not. */
    <CheckboxPrimitive.Root
        ref={ref}
        className={cn(
            "peer relative h-4 w-4 shrink-0 rounded-[4px] border border-primary shadow-sm before:absolute before:-inset-1 before:content-[''] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:bg-primary data-[state=checked]:text-primary-foreground data-[state=indeterminate]:bg-primary data-[state=indeterminate]:text-primary-foreground",
            className
        )}
        {...props}
    >
        {/* THE THIRD STATE HAS ITS OWN MARK. Radix drives `data-state="indeterminate"` and renders
            the Indicator for it exactly as it does for "checked", so a single `<Check/>` in here
            made a partial selection look identical to a complete one — the control would claim
            every item was ticked while half of them were not. No caller passed `indeterminate`
            before the chart studio's Select-all did, so nothing that exists today changes shape;
            what changes is that the state is now drawable at all. */}
        <CheckboxPrimitive.Indicator
            className={cn("flex items-center justify-center text-current")}
        >
            {props.checked === "indeterminate"
                ? <Minus className="h-4 w-4" />
                : <Check className="h-4 w-4" />}
        </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Root>
))
Checkbox.displayName = CheckboxPrimitive.Root.displayName

export { Checkbox }
