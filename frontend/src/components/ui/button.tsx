import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * `focus-visible:ring-ring` at FULL alpha, where shadcn ships `ring-ring/50`.
 *
 * shadcn's newer focus style is two parts — a solid `border-ring` plus a soft
 * half-alpha `ring-[3px]` halo — and on `Input`, `Badge` and `SelectTrigger` that
 * works, because those components carry a border-width at rest, so the solid half
 * is what carries the contrast and the halo is decoration.
 *
 * Button is the exception, and it is the exception on the control that appears
 * most: its `default`, `ghost` and `link` variants set no border-width at all.
 * Measured in Chrome, `border-top-width` on a focused header button is `0px` — so
 * `focus-visible:border-ring` sets a colour on a border that is never painted, and
 * the halo is not decoration, it is the ENTIRE indicator. Scanned out of a
 * screenshot, that halo was `#cfcfd3` on white (1.54:1) and `#44444d` on the dark
 * header (1.83:1), against a 3:1 bar (WCAG 1.4.11 Non-text Contrast — the non-text
 * bar; 4.5:1 is 1.4.3 and governs text).
 *
 * Half alpha cannot be rescued by darkening `--ring`: even at a much darker token
 * the composite only reaches ~1.73:1, because it is being diluted 50% into the
 * surface. So the alpha goes rather than the geometry. The visual cost is that the
 * button's focus ring is now a solid 3px band instead of a soft one — heavier than
 * stock shadcn, and deliberately so.
 *
 * Left alone on purpose: `ring-ring/50` on Input, Badge and SelectTrigger, whose
 * solid `border-ring` does carry the indicator once `--ring` itself passes.
 */
const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-all cursor-pointer disabled:cursor-not-allowed disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-4 shrink-0 [&_svg]:shrink-0 outline-none focus-visible:border-ring focus-visible:ring-ring focus-visible:ring-[3px] aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground shadow-xs hover:bg-primary/90",
        destructive:
          "bg-destructive text-white shadow-xs hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:focus-visible:ring-destructive/40 dark:bg-destructive/60",
        outline:
          "border bg-background shadow-xs hover:bg-accent hover:text-accent-foreground dark:bg-input/30 dark:border-input dark:hover:bg-input/50",
        secondary:
          "bg-secondary text-secondary-foreground shadow-xs hover:bg-secondary/80",
        ghost:
          "hover:bg-accent hover:text-accent-foreground dark:hover:bg-accent/50",
        link: "text-primary underline-offset-4 hover:underline",
        ai: "bg-gradient-to-br from-purple-500 via-pink-500 to-orange-500 text-white hover:shadow-xs transition-all duration-200 hover:from-purple-600 hover:via-pink-600 hover:to-orange-600 [&_svg]:text-white",
      },
      size: {
        default: "h-9 px-4 py-2 has-[>svg]:px-3",
        sm: "h-8 rounded-md gap-1.5 px-3 has-[>svg]:px-2.5",
        lg: "h-10 rounded-md px-6 has-[>svg]:px-4",
        icon: "size-9",
        iconSm: "size-6",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

type ButtonProps = React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean;
  };

// forwardRef so Radix `asChild` (Slot) can attach the trigger ref under React 18 —
// without it, components like DropdownMenuTrigger can't anchor their popover to the button.
const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";

    return (
      <Comp
        ref={ref}
        data-slot="button"
        className={cn(buttonVariants({ variant, size, className }))}
        {...props}
      />
    );
  },
);
Button.displayName = "Button";

export { Button, buttonVariants };
