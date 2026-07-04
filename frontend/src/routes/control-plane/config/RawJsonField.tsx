import { useEffect, useRef, useState } from "react";

import { Textarea } from "@/components/ui/textarea";
import { FieldLabel } from "./FieldLabel";
import { useValidity } from "./validityRegistry";

/**
 * The one raw-JSON editor for the config UI. Holds its own editable text (seeded
 * once from the parsed `value`); on each edit it debounces a `JSON.parse` and,
 * when the text parses, reports the parsed value up via `onChange` — empty text
 * parses to `undefined`. While the text is invalid it shows an inline error and
 * registers `id` as invalid in the surrounding `ValidityProvider` (null-safe, so
 * it also works with no provider) so Save can be blocked. The invalid flag is
 * cleared on unmount so a hidden/removed field never keeps Save disabled.
 */
export function RawJsonField({
  id,
  label,
  fieldKey,
  value,
  onChange,
  placeholder,
}: {
  id: string;
  label: string;
  fieldKey?: string;
  value: unknown;
  onChange: (parsed: unknown) => void;
  placeholder?: string;
}) {
  const validity = useValidity();
  const [text, setText] = useState(() =>
    value == null ? "" : JSON.stringify(value, null, 2),
  );
  const [error, setError] = useState<string | null>(null);

  // Latest callbacks/id via refs so the debounce timer never captures a stale
  // closure and the unmount cleanup can clear the right flag.
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const setInvalidRef = useRef(validity?.setInvalid);
  setInvalidRef.current = validity?.setInvalid;
  const idRef = useRef(id);
  idRef.current = id;

  function handleText(next: string) {
    setText(next);
  }

  // Debounce the parse ~300ms after typing settles.
  useEffect(() => {
    const timer = setTimeout(() => {
      const t = text.trim();
      if (t === "") {
        setError(null);
        setInvalidRef.current?.(idRef.current, false);
        onChangeRef.current(undefined);
        return;
      }
      try {
        const parsed = JSON.parse(t);
        setError(null);
        setInvalidRef.current?.(idRef.current, false);
        onChangeRef.current(parsed);
      } catch (e) {
        setError((e as Error).message || "Invalid JSON");
        setInvalidRef.current?.(idRef.current, true);
      }
    }, 300);
    return () => clearTimeout(timer);
  }, [text]);

  // Clear this field's invalid flag when it unmounts.
  useEffect(() => {
    return () => setInvalidRef.current?.(idRef.current, false);
  }, []);

  return (
    <div className="space-y-1.5">
      <FieldLabel fieldKey={fieldKey} label={label} htmlFor={id} />
      <Textarea
        id={id}
        value={text}
        rows={4}
        className="font-mono text-xs"
        placeholder={placeholder}
        onChange={(e) => handleText(e.target.value)}
      />
      {error && <p className="text-xs text-destructive">Invalid JSON: {error}</p>}
    </div>
  );
}
