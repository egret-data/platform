import * as DialogPrimitive from "@radix-ui/react-dialog";
import { XIcon } from "lucide-react";
import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * A panel that slides in from the right edge, over an overlay: a Radix dialog,
 * so it traps focus, closes on Escape and on an outside click, and hands focus
 * back on close (`onCloseAutoFocus` to choose where).
 *
 * Unlike `DialogContent`, it does not scroll its children for the caller. It is
 * a full-height flex column: a caller keeps its header and toolbar fixed with
 * `shrink-0` and gives ONE child `min-h-0 flex-1 overflow-auto`, so that what
 * must stay reachable (a title, a picker, Download) never scrolls away with a
 * long body. The slide is the `sheet-in-right`/`sheet-out-right` keyframes in
 * `index.css`, which the reduced-motion block there neutralises (FE-A11Y-5).
 */
function Sheet(props: React.ComponentProps<typeof DialogPrimitive.Root>) {
  return <DialogPrimitive.Root data-slot="sheet" {...props} />;
}

const SheetContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content>
>(({ className, children, ...props }, ref) => (
  <DialogPrimitive.Portal>
    <DialogPrimitive.Overlay data-slot="sheet-overlay" className="fixed inset-0 z-50 bg-black/50" />
    <DialogPrimitive.Content
      ref={ref}
      data-slot="sheet-content"
      className={cn(
        "bg-background fixed inset-y-0 right-0 z-50 flex h-dvh w-full max-w-2xl flex-col gap-3 border-l p-6 shadow-lg",
        "data-[state=open]:animate-[sheet-in-right_200ms_ease-out] data-[state=closed]:animate-[sheet-out-right_150ms_ease-in]",
        className,
      )}
      {...props}
    >
      {children}
      <DialogPrimitive.Close
        data-slot="sheet-close"
        className="bg-background absolute top-4 right-4 rounded-xs opacity-70 transition-opacity hover:opacity-100 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none [&_svg]:size-4"
      >
        <XIcon aria-hidden />
        <span className="sr-only">Close</span>
      </DialogPrimitive.Close>
    </DialogPrimitive.Content>
  </DialogPrimitive.Portal>
));
SheetContent.displayName = "SheetContent";

function SheetHeader({ className, ...props }: React.ComponentProps<"div">) {
  return <div data-slot="sheet-header" className={cn("flex shrink-0 flex-col gap-1.5 pr-8", className)} {...props} />;
}

function SheetTitle({ className, ...props }: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return <DialogPrimitive.Title data-slot="sheet-title" className={cn("text-lg leading-none font-semibold", className)} {...props} />;
}

function SheetDescription({ className, ...props }: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description data-slot="sheet-description" className={cn("text-muted-foreground text-sm", className)} {...props} />
  );
}

export { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle };
