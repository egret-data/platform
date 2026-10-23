import { lazy, Suspense, useId, useState } from "react";
import dayjs from "dayjs";
import { CalendarIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

// The month grid only loads when a picker opens, so a page that shows a
// picker does not pull the calendar into its first paint.
const Calendar = lazy(() => import("@/components/ui/calendar").then((m) => ({ default: m.Calendar })));

/** How a chosen instant reads, the same in every browser and locale. */
export const DATE_TIME_FORMAT = "YYYY-MM-DD HH:mm";

const TIME = /^([01]?\d|2[0-3]):([0-5]\d)$/;

/** `"HH:mm"` → `[hours, minutes]`, or null when it is not a time of day. */
function parseTime(text: string): [number, number] | null {
  const m = TIME.exec(text.trim());
  return m ? [Number(m[1]), Number(m[2])] : null;
}

interface Props {
  /** Ties a `Label htmlFor` to the trigger. */
  id?: string;
  /** The chosen instant (local wall time to the minute), or null for none. */
  value: Date | null;
  onChange: (next: Date | null) => void;
  /** The time a day gets when it is picked before any time was set, `"HH:mm"`. */
  defaultTime?: string;
  placeholder?: string;
  className?: string;
}

/**
 * A date and a time of day, picked from the app's own calendar instead of the
 * browser's `datetime-local`, whose format follows the browser's locale and
 * whose popup differs in every browser (KI-218). The trigger shows
 * `YYYY-MM-DD HH:mm`; the popover holds the month grid, a time field and Clear.
 */
export function DateTimePicker({
  id,
  value,
  onChange,
  defaultTime = "00:00",
  placeholder = "Any time",
  className,
}: Props) {
  const timeId = useId();
  const valueId = useId();
  const errorId = useId();
  const [open, setOpen] = useState(false);
  const [timeText, setTimeText] = useState(value ? dayjs(value).format("HH:mm") : defaultTime);
  const [timeError, setTimeError] = useState(false);

  // The time field follows the value whenever the value changes, from here or
  // from the caller: adjusted during render, React's pattern for state derived
  // from a prop (an effect would paint the stale text first).
  const [shownFor, setShownFor] = useState<number | null>(value ? value.getTime() : null);
  const valueKey = value ? value.getTime() : null;
  if (valueKey !== shownFor) {
    setShownFor(valueKey);
    setTimeText(value ? dayjs(value).format("HH:mm") : defaultTime);
    setTimeError(false);
  }

  const at = (day: Date, time: [number, number]) =>
    dayjs(day).hour(time[0]).minute(time[1]).second(0).millisecond(0).toDate();

  const pickDay = (day: Date | undefined) => {
    if (!day) return;
    onChange(at(day, parseTime(timeText) ?? parseTime(defaultTime) ?? [0, 0]));
  };

  const commitTime = () => {
    const time = parseTime(timeText);
    setTimeError(time === null);
    if (time && value) onChange(at(value, time));
  };

  const shown = value ? dayjs(value).format(DATE_TIME_FORMAT) : null;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          id={id}
          type="button"
          variant="outline"
          aria-describedby={valueId}
          className={cn("h-10 w-52 justify-start font-normal tabular-nums", !shown && "text-muted-foreground", className)}
        >
          <CalendarIcon aria-hidden="true" />
          <span id={valueId}>{shown ?? placeholder}</span>
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-auto space-y-3 p-3">
        <Suspense fallback={<Skeleton className="h-72 w-64" />}>
          <Calendar
            mode="single"
            selected={value ?? undefined}
            defaultMonth={value ?? undefined}
            onSelect={pickDay}
            autoFocus
          />
        </Suspense>
        <div className="space-y-1.5">
          <Label htmlFor={timeId}>Time</Label>
          <div className="flex items-center gap-2">
            <Input
              id={timeId}
              value={timeText}
              placeholder="HH:mm"
              inputMode="numeric"
              disabled={!value}
              aria-invalid={timeError || undefined}
              aria-describedby={timeError ? errorId : undefined}
              className="w-24 tabular-nums"
              onChange={(e) => setTimeText(e.target.value)}
              onBlur={commitTime}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  commitTime();
                }
              }}
            />
            <Button
              type="button"
              variant="ghost"
              className="ml-auto"
              disabled={!value}
              onClick={() => {
                onChange(null);
                setOpen(false);
              }}
            >
              Clear
            </Button>
          </div>
          {timeError && (
            <p id={errorId} className="text-xs text-destructive">
              Enter a time as HH:mm, from 00:00 to 23:59.
            </p>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}
