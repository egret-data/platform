import type { ComponentProps } from "react";
import { DayPicker } from "react-day-picker";
import { ChevronLeft, ChevronRight } from "lucide-react";

import { buttonVariants } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * A month grid, drawn with the app's tokens. `react-day-picker` brings the
 * keyboard grid (arrows, Home/End, PageUp/PageDown) and the labels; its own
 * stylesheet is not loaded, every class comes from here.
 */
export function Calendar({ className, classNames, ...props }: ComponentProps<typeof DayPicker>) {
  return (
    <DayPicker
      showOutsideDays
      className={cn("p-1", className)}
      classNames={{
        months: "relative flex flex-col",
        month: "space-y-3",
        month_caption: "flex h-8 items-center justify-center",
        caption_label: "text-sm font-medium",
        nav: "absolute inset-x-0 top-0 flex items-center justify-between",
        button_previous: cn(buttonVariants({ variant: "ghost", size: "icon" }), "size-8"),
        button_next: cn(buttonVariants({ variant: "ghost", size: "icon" }), "size-8"),
        month_grid: "w-full border-collapse",
        weekdays: "flex",
        weekday: "w-8 text-xs font-normal text-muted-foreground",
        week: "mt-1 flex w-full",
        day: "size-8 p-0 text-center text-sm",
        day_button: cn(
          buttonVariants({ variant: "ghost", size: "icon" }),
          "size-8 font-normal tabular-nums aria-selected:opacity-100",
        ),
        selected:
          "[&>button]:bg-primary [&>button]:text-primary-foreground [&>button]:hover:bg-primary [&>button]:hover:text-primary-foreground",
        today: "[&>button]:font-semibold [&>button]:underline",
        outside: "text-muted-foreground opacity-50",
        disabled: "text-muted-foreground opacity-50",
        hidden: "invisible",
        ...classNames,
      }}
      components={{
        Chevron: ({ orientation }) =>
          orientation === "left" ? <ChevronLeft className="size-4" /> : <ChevronRight className="size-4" />,
      }}
      {...props}
    />
  );
}
