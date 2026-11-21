import { lazy, Suspense, useId, useState } from "react";
import dayjs from "dayjs";
import { CalendarIcon, ChevronLeft, ChevronRight } from "lucide-react";

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

/** How finely a picker keeps the time of day. */
export type TimePrecision = "minute" | "second" | "millisecond";

const TIME_FORMAT: Record<TimePrecision, string> = {
  minute: "HH:mm",
  second: "HH:mm:ss",
  millisecond: "HH:mm:ss.SSS",
};
const TIME_HINT: Record<TimePrecision, string> = {
  minute: "HH:mm, from 00:00 to 23:59",
  second: "HH:mm:ss, from 00:00:00 to 23:59:59",
  millisecond: "HH:mm:ss.SSS, from 00:00:00.000 to 23:59:59.999",
};
const TIME = /^([01]?\d|2[0-3]):([0-5]\d)(?::([0-5]\d)(?:\.(\d{1,3}))?)?$/;

/** `[hours, minutes, seconds, milliseconds]` of a time of day. */
type Time = [number, number, number, number];

/**
 * `"HH:mm[:ss[.SSS]]"` → a time of day, or null when it is not one. Parts finer
 * than `precision` are dropped, so a minute picker keeps whole minutes.
 */
function parseTime(text: string, precision: TimePrecision = "minute"): Time | null {
  const m = TIME.exec(text.trim());
  if (!m) return null;
  const seconds = precision === "minute" ? 0 : Number(m[3] ?? 0);
  const millis = precision === "millisecond" && m[4] ? Number(m[4].padEnd(3, "0")) : 0;
  return [Number(m[1]), Number(m[2]), seconds, millis];
}

interface Props {
  /** Ties a `Label htmlFor` to the trigger. */
  id?: string;
  /** The chosen instant (local wall time to the minute), or null for none. */
  value: Date | null;
  onChange: (next: Date | null) => void;
  /** The time a day gets when it is picked before any time was set, `"HH:mm"`. */
  defaultTime?: string;
  /** How finely the time is kept (default: to the minute). */
  precision?: TimePrecision;
  /** The earliest and latest instant that can be picked. */
  min?: Date;
  max?: Date;
  placeholder?: string;
  className?: string;
  "aria-label"?: string;
  "aria-invalid"?: boolean;
  /** Ids of help or error text; the trigger's own value is always described too. */
  "aria-describedby"?: string;
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
  precision = "minute",
  min,
  max,
  placeholder = "Any time",
  className,
  "aria-label": ariaLabel,
  "aria-invalid": ariaInvalid,
  "aria-describedby": ariaDescribedBy,
}: Props) {
  const timeFormat = TIME_FORMAT[precision];
  const timeId = useId();
  const valueId = useId();
  const errorId = useId();
  const [open, setOpen] = useState(false);
  const [timeText, setTimeText] = useState(value ? dayjs(value).format(timeFormat) : defaultTime);
  const [timeError, setTimeError] = useState(false);

  // The time field follows the value whenever the value changes, from here or
  // from the caller: adjusted during render, React's pattern for state derived
  // from a prop (an effect would paint the stale text first).
  const [shownFor, setShownFor] = useState<number | null>(value ? value.getTime() : null);
  const valueKey = value ? value.getTime() : null;
  if (valueKey !== shownFor) {
    setShownFor(valueKey);
    setTimeText(value ? dayjs(value).format(timeFormat) : defaultTime);
    setTimeError(false);
  }

  const at = (day: Date, time: Time) =>
    dayjs(day).hour(time[0]).minute(time[1]).second(time[2]).millisecond(time[3]).toDate();

  const pickDay = (day: Date | undefined) => {
    if (!day) return;
    onChange(at(day, parseTime(timeText, precision) ?? parseTime(defaultTime, precision) ?? [0, 0, 0, 0]));
  };

  const commitTime = () => {
    const time = parseTime(timeText, precision);
    setTimeError(time === null);
    if (time && value) onChange(at(value, time));
  };

  const shown = value ? dayjs(value).format(`YYYY-MM-DD ${timeFormat}`) : null;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          id={id}
          type="button"
          variant="outline"
          aria-label={ariaLabel}
          aria-describedby={ariaDescribedBy ? `${valueId} ${ariaDescribedBy}` : valueId}
          className={cn(
            "h-10 w-52 justify-start font-normal tabular-nums",
            !shown && "text-muted-foreground",
            // A button has no `aria-invalid`; the error text it is described by carries the message.
            ariaInvalid && "border-destructive",
            className,
          )}
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
            disabled={[...(min ? [{ before: dayjs(min).startOf("day").toDate() }] : []), ...(max ? [{ after: max }] : [])]}
            autoFocus
          />
        </Suspense>
        <div className="space-y-1.5">
          <Label htmlFor={timeId}>Time</Label>
          <div className="flex items-center gap-2">
            <Input
              id={timeId}
              value={timeText}
              placeholder={timeFormat}
              inputMode="numeric"
              disabled={!value}
              aria-invalid={timeError || undefined}
              aria-describedby={timeError ? errorId : undefined}
              className={cn("tabular-nums", precision === "minute" ? "w-24" : "w-36")}
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
              Enter a time as {TIME_HINT[precision]}.
            </p>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** `"YYYY-MM-DD"` → a local Date at midnight, or null when it is not a date. */
function parseDay(text: string): Date | null {
  const d = dayjs(text, "YYYY-MM-DD", true);
  return text && d.isValid() ? d.toDate() : null;
}

interface FieldProps {
  id?: string;
  className?: string;
  placeholder?: string;
  "aria-label"?: string;
  "aria-invalid"?: boolean;
  "aria-describedby"?: string;
}

/**
 * A calendar day as a `"YYYY-MM-DD"` string ("" for none): what a native
 * native date input held, picked from the app's calendar instead. `min` and
 * `max` are `"YYYY-MM-DD"` too.
 */
export function DateField({
  value,
  onChange,
  min,
  max,
  placeholder = "Pick a date",
  className,
  "aria-invalid": invalid,
  ...aria
}: FieldProps & { value: string; onChange: (next: string) => void; min?: string; max?: string }) {
  const [open, setOpen] = useState(false);
  const day = parseDay(value);
  const minDay = min ? parseDay(min) : null;
  const maxDay = max ? parseDay(max) : null;
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="outline"
          {...aria}
          className={cn("h-10 w-full justify-start font-normal tabular-nums", !day && "text-muted-foreground", invalid && "border-destructive", className)}
        >
          <CalendarIcon aria-hidden="true" />
          <span>{day ? value : placeholder}</span>
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-auto space-y-2 p-3">
        <Suspense fallback={<Skeleton className="h-72 w-64" />}>
          <Calendar
            mode="single"
            selected={day ?? undefined}
            defaultMonth={day ?? undefined}
            onSelect={(d) => {
              if (!d) return;
              onChange(dayjs(d).format("YYYY-MM-DD"));
              setOpen(false);
            }}
            disabled={[...(minDay ? [{ before: minDay }] : []), ...(maxDay ? [{ after: maxDay }] : [])]}
            autoFocus
          />
        </Suspense>
        <Button
          type="button"
          variant="ghost"
          disabled={!day}
          onClick={() => {
            onChange("");
            setOpen(false);
          }}
        >
          Clear
        </Button>
      </PopoverContent>
    </Popover>
  );
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** A month as a `"YYYY-MM"` string ("" for none): what the native month input held. */
export function MonthField({
  value,
  onChange,
  placeholder = "Pick a month",
  className,
  "aria-invalid": invalid,
  ...aria
}: FieldProps & { value: string; onChange: (next: string) => void }) {
  const [open, setOpen] = useState(false);
  const m = /^(\d{4})-(0[1-9]|1[0-2])$/.exec(value);
  const [year, setYear] = useState(m ? Number(m[1]) : new Date().getFullYear());
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="outline"
          {...aria}
          className={cn("h-10 w-full justify-start font-normal tabular-nums", !m && "text-muted-foreground", invalid && "border-destructive", className)}
        >
          <CalendarIcon aria-hidden="true" />
          <span>{m ? value : placeholder}</span>
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-56 space-y-2 p-3">
        <div className="flex items-center justify-between">
          <Button type="button" variant="ghost" size="sm" aria-label="Previous year" onClick={() => setYear(year - 1)}>
            <ChevronLeft aria-hidden="true" />
          </Button>
          <span className="text-sm font-medium tabular-nums">{year}</span>
          <Button type="button" variant="ghost" size="sm" aria-label="Next year" onClick={() => setYear(year + 1)}>
            <ChevronRight aria-hidden="true" />
          </Button>
        </div>
        <div className="grid grid-cols-3 gap-1">
          {MONTHS.map((name, i) => {
            const key = `${year}-${String(i + 1).padStart(2, "0")}`;
            return (
              <Button
                key={name}
                type="button"
                size="sm"
                variant={key === value ? "default" : "ghost"}
                onClick={() => {
                  onChange(key);
                  setOpen(false);
                }}
              >
                {name}
              </Button>
            );
          })}
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** `"YYYY-MM-DDTHH:mm[:ss[.SSS]]"` (a `datetime-local` value) → a local Date. */
function parseLocal(text: string): Date | null {
  const d = dayjs(text);
  return text && d.isValid() ? d.toDate() : null;
}

/**
 * A local date and time as the `"YYYY-MM-DDTHH:mm[:ss[.SSS]]"` string a native
 * `datetime-local` input held ("" for none), picked with `DateTimePicker`.
 */
export function LocalDateTimeField({
  value,
  onChange,
  precision = "minute",
  max,
  ...rest
}: FieldProps & {
  value: string;
  onChange: (next: string) => void;
  precision?: TimePrecision;
  /** The latest instant, in the same string format. */
  max?: string;
  defaultTime?: string;
}) {
  const fmt = `YYYY-MM-DDTHH:${TIME_FORMAT[precision].slice(3)}`;
  return (
    <DateTimePicker
      {...rest}
      precision={precision}
      max={max ? (parseLocal(max) ?? undefined) : undefined}
      value={parseLocal(value)}
      onChange={(d) => onChange(d ? dayjs(d).format(fmt) : "")}
    />
  );
}

/** A time of day as `"HH:mm[:ss]"` ("" for none), typed into the shared `Input`. */
export function TimeField({
  value,
  onChange,
  seconds = false,
  className,
  ...aria
}: FieldProps & { value: string; onChange: (next: string) => void; seconds?: boolean }) {
  return (
    <Input
      {...aria}
      inputMode="numeric"
      autoComplete="off"
      placeholder={seconds ? "HH:mm:ss" : "HH:mm"}
      className={cn("tabular-nums", className)}
      value={value}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}
