import * as React from "react"
import * as CheckboxPrimitive from "@radix-ui/react-checkbox"
import { Check, Minus } from "lucide-react"

import { cn } from "@/lib/utils"

const Checkbox = React.forwardRef<
    React.ElementRef<typeof CheckboxPrimitive.Root>,
    React.ComponentPropsWithoutRef<typeof CheckboxPrimitive.Root>
>(({ className, ...props }, ref) => (
    <CheckboxPrimitive.Root
        ref={ref}
        className={cn(
            "peer h-4 w-4 shrink-0 rounded-sm border border-primary shadow-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:bg-primary data-[state=checked]:text-primary-foreground data-[state=indeterminate]:bg-primary data-[state=indeterminate]:text-primary-foreground",
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
