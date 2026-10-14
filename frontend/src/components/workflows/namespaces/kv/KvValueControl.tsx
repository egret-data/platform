import { useEffect, useId, useState } from "react";
import MonacoEditor from "@/components/monaco/MonacoEditor";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useIsDark } from "@/hooks/useIsDark";
import { encodeValue, KV_TYPE_HELP, type KvType } from "./kvValue";

/**
 * The value's control for its type (W7d spec §4.4): text for a STRING, a
 * number field that keeps every digit, a true/false picker, the browser's
 * date-time and date pickers, a duration field, and Monaco for JSON. The
 * label is "Value" for each, the type's help under it, and the error (from
 * `encodeValue`, the server's rules) beside it (FE-FORM-4).
 */
export function KvValueControl({
  type,
  value,
  onChange,
  error,
}: {
  type: KvType;
  value: string;
  onChange: (value: string) => void;
  error: string | null;
}) {
  const isDark = useIsDark();
  const modelId = useId().replace(/:/g, "");
  // Monaco's own input (a textarea inside its node) is what a screen reader is
  // on while editing JSON, so the help and the error are tied to it there.
  const [editorNode, setEditorNode] = useState<HTMLElement | null>(null);
  const describedBy = `kv-value-help${error ? " kv-value-error" : ""}`;
  const common = { id: "kv-value", "aria-invalid": error ? true : undefined, "aria-describedby": describedBy };
  useEffect(() => {
    editorNode?.querySelector("textarea")?.setAttribute("aria-describedby", describedBy);
  });

  let control;
  switch (type) {
    case "STRING":
      control = <Textarea {...common} rows={4} value={value} onChange={(e) => onChange(e.target.value)} />;
      break;
    case "NUMBER":
      // Text, not `type="number"`: a number field rounds to a double, and the server keeps every digit.
      control = <Input {...common} inputMode="decimal" autoComplete="off" value={value} onChange={(e) => onChange(e.target.value)} />;
      break;
    case "BOOLEAN":
      control = (
        <Select value={value || undefined} onValueChange={onChange}>
          <SelectTrigger {...common} className="w-40">
            <SelectValue placeholder="Choose…" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="true">true</SelectItem>
            <SelectItem value="false">false</SelectItem>
          </SelectContent>
        </Select>
      );
      break;
    case "DATETIME": {
      const utc = encodeValue("DATETIME", value);
      control = (
        <>
          <Input {...common} type="datetime-local" step="0.001" className="w-64" value={value} onChange={(e) => onChange(e.target.value)} />
          {utc.ok ? <p className="text-xs text-muted-foreground tabular-nums">Stored as {utc.body} (UTC).</p> : null}
        </>
      );
      break;
    }
    case "DATE":
      control = <Input {...common} type="date" className="w-48" value={value} onChange={(e) => onChange(e.target.value)} />;
      break;
    case "DURATION":
      control = (
        <Input {...common} autoComplete="off" placeholder="PT1H30M" className="w-48 font-mono" value={value} onChange={(e) => onChange(e.target.value)} />
      );
      break;
    case "JSON":
      control = (
        <div className="h-60 overflow-hidden rounded-md border">
          <MonacoEditor
            height="100%"
            language="json"
            path={`egret-kv-value-${modelId}.json`}
            // Uncontrolled: Monaco owns the text while it is open and reports each edit.
            defaultValue={value}
            theme={isDark ? "vs-dark" : "vs"}
            onChange={(v) => onChange(v ?? "")}
            onMount={(editor) => setEditorNode(editor.getDomNode())}
            options={{ ariaLabel: "Value", minimap: { enabled: false }, scrollBeyondLastLine: false, fontSize: 13, tabSize: 2 }}
          />
        </div>
      );
      break;
  }

  return (
    <div className="grid gap-1.5">
      {type === "JSON" ? (
        // Monaco takes its name from `ariaLabel`; this is its visible label.
        <span className="text-sm font-medium">Value</span>
      ) : (
        <Label htmlFor="kv-value">Value</Label>
      )}
      {control}
      <p id="kv-value-help" className="text-xs text-muted-foreground">
        {KV_TYPE_HELP[type]}
      </p>
      {/* Live: an error that appears on Save is announced, whichever control has focus. */}
      <div aria-live="polite">
        {error ? (
          <p id="kv-value-error" className="text-sm text-destructive">
            {error}
          </p>
        ) : null}
      </div>
    </div>
  );
}
