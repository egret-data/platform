import * as React from "react"
import * as DropdownMenuPrimitive from "@radix-ui/react-dropdown-menu"
import { Check, ChevronRight, Circle } from "lucide-react"

import { cn } from "@/lib/utils"

/**
 * THE FOCUS INDICATOR FOR A MENU ITEM — `focus-visible:ring-2
 * focus-visible:ring-ring focus-visible:ring-inset`, on all four item types
 * below and on an identical copy in `select.tsx`'s `SelectItem`. Written out at
 * each site rather than hoisted into a shared constant, because the assertion
 * that pins it reads the class strings this file actually ships
 * (`src/__tests__/focusIndicatorContrast.test.ts`) and a constant would put a
 * TypeScript resolver between the test and the truth. The rationale lives here,
 * once.
 *
 * These items opt out of the outline (`outline-none`) and, until this was added,
 * announced focus by swapping their background to `--accent` and nothing else.
 * That swap measures 1.03:1 in light and 1.18:1 in dark against the `--popover`
 * they sit on. The bar is 3:1 — WCAG 2.2 **1.4.11 Non-text Contrast**, not the
 * 4.5:1 of 1.4.3, which governs text.
 *
 * WHY NOT JUST DARKEN `--accent`. It was the other option and it is much the
 * bigger hammer: `--accent` is read by 63 files, 55 of those occurrences are
 * `hover:bg-accent` and only 5 are the `focus:` variant this defect is about. It
 * is the product's resting hover background — sidebar rows, table rows, filter
 * pills. Reaching 3:1 would need the light token to fall from L 0.97 to 0.654 and
 * the dark one to rise from 0.27 to 0.504, i.e. a mid-grey slab under every mouse
 * hover in the product, with `--accent-foreground` then sitting on it at ~4:1 and
 * failing the text bar it does have to meet. A hover colour cannot double as a
 * focus indicator without ceasing to be a hover colour.
 *
 * This is also Metabase's shape, read out of its shipped stylesheet rather than
 * guessed at: its focusable controls are `:focus-visible { outline: 2px solid
 * var(--mb-color-focus) }`, a dedicated focus token on `:focus-visible`, with
 * `outline-offset: -2px` (or `box-shadow: inset 0 0 0 2px`) wherever the element
 * must not paint outside itself. Copied is the RULE. The value is egret's own
 * `--ring`, which `src/__tests__/focusIndicatorContrast.test.ts` already holds at
 * 3:1 on every surface including `--accent`, so the ring reads whether it lands
 * on the focused item's own background or on the popover beside it.
 *
 * `ring-inset`, because `DropdownMenuContent` and `SubContent` are
 * `overflow-hidden` with 4px of padding: a 2px outset ring on the first or last
 * item would be cropped along exactly the edge that most needs to read.
 *
 * `focus-visible:` rather than `focus:` — but NOT for the reason it looks like,
 * and the difference is recorded here because the first draft of this note got it
 * backwards and the browser said so.
 *
 * Radix moves real DOM focus onto an item on POINTER MOVE as well as on arrow-key
 * navigation; that is how `focus:bg-accent` manages to double as the hover state.
 * The draft claimed `:focus-visible` would therefore keep the ring off the mouse
 * path. IT DOES NOT. Measured in Chrome against the built bundle: hovering an
 * item with a real mousemove leaves it matching `:focus-visible`, and the ring
 * paints. The control run says the discriminator itself is working — a nav link
 * focused by a real mouse CLICK matches `:focus` and not `:focus-visible` — so
 * what is happening is that Radix focuses the item FROM SCRIPT, and Chrome treats
 * a script `focus()` that the pointer did not itself target as keyboard-ish.
 * There is no CSS-only way to tell the two apart on these elements.
 *
 * So today `focus:` and `focus-visible:` paint identically here, and the ring
 * shows under the mouse as well as under the caret. That is accepted rather than
 * worked around, on two grounds. It is not misleading: in a Radix menu the
 * highlighted item IS the one Enter will run, whichever device moved it there, so
 * the ring reports something true. And working around it would mean tracking
 * input modality on `<html>` and hanging both halves of the highlight off that —
 * product-wide machinery, with its own tests, bought for a cosmetic difference.
 *
 * `focus-visible:` is kept anyway, because it is the selector whose job this is,
 * and because the only plausible direction of browser change is browsers getting
 * BETTER at attributing pointer-driven script focus — which would quietly land
 * the keyboard-only behaviour that was wanted in the first place, where `focus:`
 * never could.
 *
 * None of this is pinned by a test. jsdom runs no layout and no paint, so
 * `:focus-visible` cannot be evaluated there at all; the assertion in
 * `focusIndicatorContrast.test.ts` pins the utilities and the contrast, and the
 * modality behaviour above rests on the measurement described, not on CI.
 */

const DropdownMenu = DropdownMenuPrimitive.Root

const DropdownMenuTrigger = DropdownMenuPrimitive.Trigger

const DropdownMenuGroup = DropdownMenuPrimitive.Group

const DropdownMenuPortal = DropdownMenuPrimitive.Portal

const DropdownMenuSub = DropdownMenuPrimitive.Sub

const DropdownMenuRadioGroup = DropdownMenuPrimitive.RadioGroup

const DropdownMenuSubTrigger = React.forwardRef<
    React.ElementRef<typeof DropdownMenuPrimitive.SubTrigger>,
    React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.SubTrigger> & {
        inset?: boolean
    }
>(({ className, inset, children, ...props }, ref) => (
    <DropdownMenuPrimitive.SubTrigger
        ref={ref}
        className={cn(
            "flex cursor-default select-none items-center rounded-sm px-2 py-1.5 text-sm outline-none focus:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset data-[state=open]:bg-accent",
            inset && "pl-8",
            className
        )}
        {...props}
    >
        {children}
        <ChevronRight className="ml-auto h-4 w-4" />
    </DropdownMenuPrimitive.SubTrigger>
))
DropdownMenuSubTrigger.displayName =
    DropdownMenuPrimitive.SubTrigger.displayName

const DropdownMenuSubContent = React.forwardRef<
    React.ElementRef<typeof DropdownMenuPrimitive.SubContent>,
    React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.SubContent>
>(({ className, ...props }, ref) => (
    <DropdownMenuPrimitive.SubContent
        ref={ref}
        className={cn(
            "z-50 min-w-[8rem] overflow-hidden rounded-md border bg-popover p-1 text-popover-foreground shadow-lg data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2",
            className
        )}
        {...props}
    />
))
DropdownMenuSubContent.displayName =
    DropdownMenuPrimitive.SubContent.displayName

const DropdownMenuContent = React.forwardRef<
    React.ElementRef<typeof DropdownMenuPrimitive.Content>,
    React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Content>
>(({ className, sideOffset = 4, ...props }, ref) => (
    <DropdownMenuPrimitive.Portal>
        <DropdownMenuPrimitive.Content
            ref={ref}
            sideOffset={sideOffset}
            className={cn(
                "z-50 min-w-[8rem] overflow-hidden rounded-md border bg-popover p-1 text-popover-foreground shadow-md data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2",
                className
            )}
            {...props}
        />
    </DropdownMenuPrimitive.Portal>
))
DropdownMenuContent.displayName = DropdownMenuPrimitive.Content.displayName

const DropdownMenuItem = React.forwardRef<
    React.ElementRef<typeof DropdownMenuPrimitive.Item>,
    React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Item> & {
        inset?: boolean
    }
>(({ className, inset, ...props }, ref) => (
    <DropdownMenuPrimitive.Item
        ref={ref}
        className={cn(
            "relative flex cursor-default select-none items-center rounded-sm px-2 py-1.5 text-sm outline-none transition-colors focus:bg-accent focus:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset data-[disabled]:pointer-events-none data-[disabled]:opacity-50",
            inset && "pl-8",
            className
        )}
        {...props}
    />
))
DropdownMenuItem.displayName = DropdownMenuPrimitive.Item.displayName

const DropdownMenuCheckboxItem = React.forwardRef<
    React.ElementRef<typeof DropdownMenuPrimitive.CheckboxItem>,
    React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.CheckboxItem>
>(({ className, children, checked, ...props }, ref) => (
    <DropdownMenuPrimitive.CheckboxItem
        ref={ref}
        className={cn(
            "relative flex cursor-default select-none items-center rounded-sm py-1.5 pl-8 pr-2 text-sm outline-none transition-colors focus:bg-accent focus:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset data-[disabled]:pointer-events-none data-[disabled]:opacity-50",
            className
        )}
        checked={checked}
        {...props}
    >
        <span className="absolute left-2 flex h-3.5 w-3.5 items-center justify-center">
            <DropdownMenuPrimitive.ItemIndicator>
                <Check className="h-4 w-4" />
            </DropdownMenuPrimitive.ItemIndicator>
        </span>
        {children}
    </DropdownMenuPrimitive.CheckboxItem>
))
DropdownMenuCheckboxItem.displayName =
    DropdownMenuPrimitive.CheckboxItem.displayName

const DropdownMenuRadioItem = React.forwardRef<
    React.ElementRef<typeof DropdownMenuPrimitive.RadioItem>,
    React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.RadioItem>
>(({ className, children, ...props }, ref) => (
    <DropdownMenuPrimitive.RadioItem
        ref={ref}
        className={cn(
            "relative flex cursor-default select-none items-center rounded-sm py-1.5 pl-8 pr-2 text-sm outline-none transition-colors focus:bg-accent focus:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset data-[disabled]:pointer-events-none data-[disabled]:opacity-50",
            className
        )}
        {...props}
    >
        <span className="absolute left-2 flex h-3.5 w-3.5 items-center justify-center">
            <DropdownMenuPrimitive.ItemIndicator>
                <Circle className="h-2 w-2 fill-current" />
            </DropdownMenuPrimitive.ItemIndicator>
        </span>
        {children}
    </DropdownMenuPrimitive.RadioItem>
))
DropdownMenuRadioItem.displayName = DropdownMenuPrimitive.RadioItem.displayName

const DropdownMenuLabel = React.forwardRef<
    React.ElementRef<typeof DropdownMenuPrimitive.Label>,
    React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Label> & {
        inset?: boolean
    }
>(({ className, inset, ...props }, ref) => (
    <DropdownMenuPrimitive.Label
        ref={ref}
        className={cn(
            "px-2 py-1.5 text-sm font-semibold",
            inset && "pl-8",
            className
        )}
        {...props}
    />
))
DropdownMenuLabel.displayName = DropdownMenuPrimitive.Label.displayName

const DropdownMenuSeparator = React.forwardRef<
    React.ElementRef<typeof DropdownMenuPrimitive.Separator>,
    React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Separator>
>(({ className, ...props }, ref) => (
    <DropdownMenuPrimitive.Separator
        ref={ref}
        className={cn("-mx-1 my-1 h-px bg-muted", className)}
        {...props}
    />
))
DropdownMenuSeparator.displayName = DropdownMenuPrimitive.Separator.displayName

const DropdownMenuShortcut = ({
    className,
    ...props
}: React.HTMLAttributes<HTMLSpanElement>) => {
    return (
        <span
            className={cn("ml-auto text-xs tracking-widest opacity-60", className)}
            {...props}
        />
    )
}
DropdownMenuShortcut.displayName = "DropdownMenuShortcut"

export {
    DropdownMenu,
    DropdownMenuTrigger,
    DropdownMenuContent,
    DropdownMenuItem,
    DropdownMenuCheckboxItem,
    DropdownMenuRadioItem,
    DropdownMenuLabel,
    DropdownMenuSeparator,
    DropdownMenuShortcut,
    DropdownMenuGroup,
    DropdownMenuPortal,
    DropdownMenuSub,
    DropdownMenuSubContent,
    DropdownMenuSubTrigger,
    DropdownMenuRadioGroup,
}
