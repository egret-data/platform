import { useState } from "react";
import type { ComponentProps } from "react";
import { Input } from "@/components/ui/input";

/**
 * A numeric settings box that can hold the number BEING TYPED into it, including a half-written
 * decimal.
 *
 * WHY THIS IS NOT `<input type="number">`. An `type=number` input whose contents are not a valid
 * number reports `value === ""` with `validity.badInput === true` — and "4." is not a valid number,
 * so the field goes empty for exactly as long as the decimal point is the last character. The
 * browser fires a real `input` event at that moment, carrying `""`, and every handler in this
 * codebase ran that string through `Number()` (which is 0) or through an `=== ""` test meaning
 * "the user cleared this". Neither is what happened: the user pressed `.`.
 *
 * Measured in Chrome 150 with real key presses (not `fill()`), typing `4` `.` `5`:
 *
 *   input#free   4 → "4"   . → "" badInput=true   5 → "4.5"
 *   input#step1  4 → "4"   . → "" badInput=true   5 → "4.5"     (step="1" min="2" max="20")
 *
 * — i.e. `step` does not gate the keystroke. `step` only decides `validity.stepMismatch`, which is
 * surfaced at form-submission time and never stops a character being typed. So the empty-string
 * event is delivered on EVERY `type=number` field, whatever its step, and "an integer field can't
 * receive a dot" is not true of any browser tested. It is only the fields where a `.` is a
 * LEGITIMATE keystroke that this control belongs on.
 *
 * The two ways that empty string then hurt, both seen in this codebase:
 *
 *   `"" -> 0`          stores a DIFFERENT SETTING. The pie's slice threshold had this: 0 means
 *                      "fold nothing", so typing 4.5 over the default went 4 → 0 → 5 and the user
 *                      who meant 4.5 got 5, with the fold silently off in between.
 *   `"" -> undefined`  CLEARS the field mid-word. Self-correcting on the next digit, but the
 *                      config is momentarily wrong and any subscriber (a live preview, an
 *                      autosave) sees the cleared state.
 *
 * HOW THIS FIXES IT. A text input, with the in-progress characters held in `draft` and a number
 * committed only once it parses. `draft === null` means "not being edited" and the box shows what
 * is stored; any string, INCLUDING `""`, is a live draft, so a genuinely emptied box is
 * distinguishable from one the browser blanked. Empty commits `undefined` — really unset — and the
 * draft is dropped on blur so the box settles on whatever the chart is actually using ("4."
 * normalises to 4, an emptied box back to its unset display).
 *
 * `inputMode="decimal"` keeps the numeric keypad on touch without handing the field back to the
 * value sanitiser that caused this.
 *
 * This is the shape Metabase's own `ChartSettingInputNumeric` uses: a `TextInput`, an `inputValue`
 * state, and `""` -> `onChange(undefined)`.
 */
export function DecimalInput({
  value,
  onChange,
  unsetText = "",
  onBlur,
  ...rest
}: {
  /** The committed number, or `undefined` for unset. */
  value: number | undefined;
  /** Called with `undefined` when the box is emptied, never with `0`. */
  onChange: (value: number | undefined) => void;
  /**
   * What the box shows when nothing is stored and nothing is being typed. Defaults to empty, so
   * the `placeholder` ("auto", "none") shows through. The pie's threshold passes its default here
   * because that control displays the number the chart is folding at rather than a hint.
   */
  unsetText?: string;
} & Omit<ComponentProps<typeof Input>, "value" | "onChange" | "type" | "inputMode">) {
  // null = "not being edited"; show what is stored. Any string, including "", is a live draft.
  const [draft, setDraft] = useState<string | null>(null);
  return (
    <Input
      {...rest}
      type="text"
      inputMode="decimal"
      value={draft ?? (value === undefined ? unsetText : String(value))}
      onChange={(e) => {
        const raw = e.target.value;
        setDraft(raw);
        if (raw.trim() === "") return onChange(undefined);
        const n = Number(raw);
        // Anything unparseable ("4.5.6", "abc", "4e") is left in the box and NOT committed — the
        // last good number stays in the config, and blur puts it back on screen. Infinity is
        // rejected the same way: `Number("Infinity")` parses but no axis bound or goal can use it.
        if (Number.isFinite(n)) onChange(n);
      }}
      onBlur={(e) => {
        setDraft(null);
        onBlur?.(e);
      }}
    />
  );
}
