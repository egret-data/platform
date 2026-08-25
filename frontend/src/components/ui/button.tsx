import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * `focus-visible:ring-2 focus-visible:ring-ring` — the product's ONE focus
 * indicator. The full reasoning, the inventory it came out of and the assertions
 * that hold it live at the bottom of `src/__tests__/focusIndicatorContrast.test.ts`.
 * Two things are worth repeating on this file specifically.
 *
 * FIRST, why the ring and not `border-ring`. shadcn's newer focus style is two
 * parts — a solid `border-ring` plus a soft half-alpha halo — and on `Input`,
 * `Badge` and `SelectTrigger` that reads, because those carry a border-width at
 * rest so the solid half carries the contrast. Button is the exception, and it is
 * the exception on the control that appears most: its `default`, `ghost` and
 * `link` variants set no border-width at all. Measured in Chrome,
 * `border-top-width` on a focused header button is `0px`, so `border-ring` sets a
 * colour on a border that is never painted and the halo was the ENTIRE indicator.
 * Scanned out of a screenshot, that halo was `#cfcfd3` on white (1.54:1) and
 * `#44444d` on the dark header (1.83:1) against a 3:1 bar (WCAG 1.4.11 Non-text
 * Contrast — the non-text bar; 4.5:1 is 1.4.3 and governs text). Half alpha
 * cannot be rescued by darkening `--ring` either: diluted 50% into the surface it
 * tops out near 1.73:1. `border-ring` stays as a second cue for the `outline`
 * variant, which does have a border, and is now never the only thing painted.
 *
 * SECOND, the destructive variant used to override this ring and lose. It carried
 * `focus-visible:ring-destructive/20 dark:focus-visible:ring-destructive/40`, and
 * because `cva` appends variant classes after the base, `twMerge` resolved the
 * `focus-visible` ring-colour conflict in the VARIANT's favour and dropped
 * `focus-visible:ring-ring` from the output entirely — verified by running
 * `twMerge` on the two strings. So every Delete/Remove button in the product was
 * focus-ringed at 20% alpha (40% in dark), which is worse than the half-alpha case
 * above and cannot reach 3:1 at any token value. The override is deleted rather
 * than made solid: a destructive button is already unmistakably red, and giving it
 * a second, differently-coloured focus indicator is exactly the divergence this
 * change exists to remove. `aria-invalid:ring-destructive/*` is left alone — that
 * is an error state, not a focus indicator, and it is a separate question.
 *
 * The width came down from #415's `ring-[3px]` to `ring-2`: 3px was chosen when
 * the halo was fighting to be seen at half alpha; solid, 2px matches the other
 * ~30 focus rings in the tree, WCAG 2.4.13's reference thickness, and Metabase's
 * own `:focus-visible { outline: 2px solid … }`.
 *
 * THIRD, and unrelated to focus: the base carries `disabled:cursor-not-allowed`
 * and NOT `disabled:pointer-events-none`. It used to carry both, in that order,
 * and the second cancelled the first. `pointer-events: none` takes the element
 * out of hit-testing, so a real browser never resolves the pointer onto it —
 * which means the `cursor` never changes AND the native `title` tooltip never
 * opens, because both are decided by hit-testing. Sixteen buttons in the tree
 * pass a `title` that only exists while they are disabled (it is the sentence
 * saying WHY), and on an icon-only button that sentence is the *only* label
 * there is. Every one of them was unreachable. Nothing in the Vitest suite could
 * see it: jsdom has no layout and no hit-testing (`docs/en/ui-conventions.md`
 * §10). It was found by a real hover, in Chrome, timing out.
 *
 * Dropping the class is safe for a real `<button>` — the `disabled` ATTRIBUTE,
 * not the class, is what blocks click, keyboard activation and focus, and it is
 * untouched. It is NOT safe for `asChild`; see the note on `inertSlot` below.
 *
 * One consequence is deliberate and worth knowing: hit-testing is also what
 * decides `:hover`, so a disabled button now matches it and a variant's hover
 * paint applies. Measured in Chrome on the `outline` variant (Config's disabled
 * Validate): background `oklch(1 0 0)` at rest against `oklch(0.97 0 0)` hovered
 * — a 3% lightness step, on an element already at `opacity-50`, next to a
 * `not-allowed` cursor and a tooltip that now open. That is the whole cost.
 *
 * It is NOT neutralised here, and the reason is a cascade fact rather than a
 * preference. `:disabled:hover` scores 0-3-0 and a call site's plain `hover:*`
 * scores 0-2-0, so any `disabled:hover:*` this component added would outrank the
 * `hover:` classes 33 call sites pass in — and it could not even compute the
 * right value to restore, because the resting paint at those call sites is
 * theirs, not the variant's (`text-current` resolves to the INHERITED colour,
 * so it would drop a `text-destructive` button's red rather than keep it). A
 * call site that minds can say `disabled:hover:…` itself, which is exact because
 * it knows its own resting value — `chart/settings/FieldLabel.tsx` already does,
 * and `ui/icon-button.tsx` can for the same reason.
 */
const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-all cursor-pointer disabled:cursor-not-allowed disabled:opacity-50 aria-disabled:cursor-not-allowed aria-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-4 shrink-0 [&_svg]:shrink-0 outline-none focus-visible:border-ring focus-visible:ring-ring focus-visible:ring-2 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground shadow-xs hover:bg-primary/90",
        destructive:
          "bg-destructive text-white shadow-xs hover:bg-destructive/90 dark:bg-destructive/60",
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

    // `disabled` is an attribute of `<button>` (and the other form controls) and
    // of NOTHING else. Rendered through `Slot` onto an `<a>` or a `<div>` — which
    // is the entire point of `asChild` — it is inert: React writes no attribute a
    // browser acts on, the element stays in the tab order, and the click fires.
    // On that path `pointer-events-none` was not decoration, it was the only
    // thing holding the button shut, and the base string no longer has it.
    //
    // So it is reapplied HERE, for that path only, unconditioned on `:disabled`
    // (which an `<a>` never matches). `aria-disabled` and `tabIndex={-1}` come
    // with it: the first is the only way the state reaches assistive tech when
    // there is no `disabled` attribute to convey it, and the second closes the
    // hole `pointer-events` cannot — it stops a keyboard reaching an element the
    // pointer can't, which the class alone never did.
    //
    // There are currently zero `<Button asChild disabled>` in `src/` (scanned
    // brace-aware over all 448 `Button`/`IconButton` tags — 22 use `asChild`,
    // none of them with `disabled`, none with a spread that could hide one), so
    // this guards a case that does not exist yet rather than fixing a live bug.
    // That is the point: without it, the first person to write one gets a control
    // that looks disabled and works.
    const inertSlot = asChild && Boolean(props.disabled);

    return (
      <Comp
        ref={ref}
        data-slot="button"
        className={cn(
          buttonVariants({ variant, size, className }),
          inertSlot && "pointer-events-none",
        )}
        {...props}
        {...(inertSlot ? { "aria-disabled": true, tabIndex: -1 } : undefined)}
      />
    );
  },
);
Button.displayName = "Button";

export { Button, buttonVariants };
