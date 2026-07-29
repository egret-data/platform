import { Label } from "@/components/ui/label";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

/**
 * Auto / On / Off — the control for any display setting the renderer decides for itself when
 * nothing is stored, so `undefined` is a real, visible third state rather than "off". A
 * two-state Switch cannot represent that without contradicting the chart it controls: it would
 * read "off" while the chart drew the thing anyway.
 *
 * Shared, because it was already needed by two chart types on identical terms (cartesian, whose
 * defaults come from the data — see buildCartesianOption; and pie, whose legend and labels
 * default on because its colours are unreadable without them — see buildPieOption). It lived
 * twice, byte for byte, differing only in its doc comment. Any further chart wanting Auto/On/Off
 * takes it from here rather than growing a third copy.
 *
 * `autoHint` says what Auto actually does for THIS setting, since that is the one thing the
 * three words on screen can't convey.
 */
export function AutoToggle({
  id, label, value, onChange, autoHint,
}: { id: string; label: string; value: boolean | undefined; onChange: (v: boolean | undefined) => void; autoHint: string }) {
  const current = value === undefined ? "auto" : value ? "on" : "off";
  return (
    <div className="flex items-center justify-between gap-2">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Select value={current} onValueChange={(v) => onChange(v === "auto" ? undefined : v === "on")}>
        <SelectTrigger id={id} aria-label={label} className="h-7 w-24"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="auto" title={autoHint}>Auto</SelectItem>
          <SelectItem value="on">On</SelectItem>
          <SelectItem value="off">Off</SelectItem>
        </SelectContent>
      </Select>
    </div>
  );
}
