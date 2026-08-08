import * as DialogPrimitive from "@radix-ui/react-dialog";
import { XIcon } from "lucide-react";
import * as React from "react";

import { cn } from "@/lib/utils";

function Dialog({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Root>) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />;
}

function DialogTrigger({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Trigger>) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />;
}

function DialogPortal({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Portal>) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />;
}

function DialogClose({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Close>) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />;
}

const DialogOverlay = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Overlay
    ref={ref}
    data-slot="dialog-overlay"
    className={cn(
      "data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 fixed inset-0 z-50 bg-black/50",
      className,
    )}
    {...props}
  />
));
DialogOverlay.displayName = DialogPrimitive.Overlay.displayName;

/**
 * THE DIALOG HAS TO BE TOLD HOW TALL IT MAY BE, AND WHICH PART OF IT SCROLLS.
 *
 * ── What was wrong ──────────────────────────────────────────────────────────
 * This is a `position: fixed` box centred with `top: 50%` + `translateY(-50%)`.
 * With no maximum height it grew to its content and centred the OVERFLOW with
 * it: half above the viewport and half below. Nothing could scroll it back —
 * Radix sets `overflow: hidden` on `body` for as long as a modal is open, and
 * the content was `overflow: visible`, so `scrollHeight === clientHeight` all
 * the way up the chain and there was no scroll container anywhere to scroll.
 *
 * Measured on the kind deployment at 1280×640, on Control plane → Storage →
 * "Add connection" (a plain form, not an unusual dialog): content 750px tall at
 * `top: -55`; the close button at `top: -38 / bottom: -22`, entirely above the
 * viewport, with `document.elementFromPoint()` at its centre returning `null`;
 * the title at `top: -30`; the footer buttons at 634–670 against a 640px
 * viewport. `close.focus()` followed by `scrollIntoView()` left it at `-38`, so
 * it could not be reached by keyboard either. #441 measured the same shape on
 * the chart-type picker (1246px at `top: -173`) and capped that ONE dialog; the
 * cap belongs here, where every dialog inherits it.
 *
 * `dvh`, not `vh`: on mobile Safari `vh` resolves against the LARGEST viewport —
 * the one behind the retracted browser chrome — so a `vh` cap still overflows by
 * the height of the toolbar. The `-4rem` leaves 32px of overlay visible above and
 * below, so a capped dialog still reads as floating rather than as a full screen.
 *
 * ── Where the scroll boundary is, and why not on the content ────────────────
 * `overflow-y-auto` on the content itself would be one line shorter and would
 * make everything reachable — but the close button is `position: absolute`
 * inside it, so it would scroll away with the content. A dialog whose only
 * dismiss affordance leaves the screen the moment you read to the bottom of it
 * is the same defect wearing a smaller hat. So the content is a flex column that
 * CLIPS, the children go in an inner `dialog-body` that scrolls, and the close
 * button is a sibling of that body — pinned by construction. This is the shape
 * #441 chose for the picker (`shrink-0` header, scrolling gallery), generalised.
 *
 * The header and footer still scroll with the body. Pinning those would mean the
 * primitive picking children apart by component type, which breaks the moment a
 * caller wraps a header in a conditional, a fragment or a layout div — and eight
 * of the 65 call sites already do. `DialogHeader` can pin itself with
 * `sticky top-0` where a specific dialog wants it; the close button, which the
 * primitive owns, is pinned for everyone.
 *
 * ── `gap-4` and `p-6` ───────────────────────────────────────────────────────
 * The gap moves to the body, because that is where the children now are; leaving
 * it on the content would silently collapse the spacing of every dialog. The
 * padding stays on the CONTENT on purpose: with it on the body, the scrollbar
 * would run to the dialog's own edge and sit under the close button, and text
 * would clip flush against the border. Keeping it outside insets the scroll
 * region by 24px, which is also what keeps the close button clear of it.
 *
 * `bodyClassName` exists for the two dialogs that manage their own scrolling
 * inside the body (the chart-type gallery, whose search field stays put while
 * the cards scroll, and the command palette, which has no gaps). It is
 * deliberately a separate prop rather than something merged out of `className`:
 * `overflow-hidden` and `overflow-y-auto` are different tailwind-merge groups,
 * so neither removes the other and which one wins depends on the order Tailwind
 * happens to emit its rules — a coin toss, not a decision.
 */
const DialogContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content> & {
    showCloseButton?: boolean;
    /** Extra classes for the scrolling body that wraps `children`. */
    bodyClassName?: string;
  }
>(({ className, children, showCloseButton = true, bodyClassName, ...props }, ref) => (
  <DialogPortal data-slot="dialog-portal">
    <DialogOverlay />
    <DialogPrimitive.Content
      ref={ref}
      data-slot="dialog-content"
      className={cn(
        "bg-background data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 fixed top-[50%] left-[50%] z-50 flex max-h-[calc(100dvh-4rem)] w-full max-w-xl translate-x-[-50%] translate-y-[-50%] flex-col overflow-hidden rounded-lg border p-6 shadow-lg duration-200",
        className,
      )}
      {...props}
    >
      {/* `min-h-0` is the load-bearing half of this: a flex child will not go
          below its own content height without it, so the body would refuse to
          shrink and push the overflow straight back out of the capped content. */}
      <div
        data-slot="dialog-body"
        className={cn("grid min-h-0 flex-1 gap-4 overflow-y-auto", bodyClassName)}
      >
        {children}
      </div>
      {showCloseButton && (
        <DialogPrimitive.Close
          data-slot="dialog-close"
          className={cn(
            // `bg-background` is not decoration: it is the same colour as the dialog, so
            // it changes nothing while a dialog is short. It exists because the body now
            // scrolls UNDER this button — the close sits 17px from the top and the body's
            // padding box starts at 25px, an 8px overlap — and without an opaque backing
            // the scrolled content shows through the icon.
            "data-[state=open]:bg-accent data-[state=open]:text-muted-foreground bg-background absolute top-4 right-4 rounded-xs opacity-70 transition-opacity hover:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
            "cursor-pointer disabled:cursor-not-allowed")}
        >
          <XIcon />
          <span className="sr-only">Close</span>
        </DialogPrimitive.Close>
      )}
    </DialogPrimitive.Content>
  </DialogPortal>
));
DialogContent.displayName = DialogPrimitive.Content.displayName;

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("flex flex-col gap-2 text-center sm:text-left", className)}
      {...props}
    />
  );
}

function DialogFooter({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn(
        "flex flex-col-reverse gap-2 sm:flex-row sm:justify-end",
        className,
      )}
      {...props}
    />
  );
}

function DialogTitle({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn("text-lg leading-none font-semibold", className)}
      {...props}
    />
  );
}

function DialogDescription({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn("text-muted-foreground text-sm", className)}
      {...props}
    />
  );
}

export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger
};
