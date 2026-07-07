import { useState } from "react";
import { Check, Pencil, X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";

interface PermissionPickerCellProps {
  value: string[];
  options: string[];
  onChange: (value: string[]) => void;
}

/**
 * Ranger-style click-to-popover Permissions cell: shows badges (or an
 * "Add permissions" placeholder) when closed, and a buffered checkbox list
 * when open — selections only take effect on ✓ (discarded on ✗ or outside
 * click without confirming).
 */
export function PermissionPickerCell({ value, options, onChange }: PermissionPickerCellProps) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<string[]>(value);

  const openWith = (next: boolean) => {
    if (next) setDraft(value);
    setOpen(next);
  };

  const toggle = (opt: string, checked: boolean) =>
    setDraft((d) => (checked ? [...d, opt] : d.filter((x) => x !== opt)));

  const allSelected = options.length > 0 && options.every((o) => draft.includes(o));
  const toggleAll = (checked: boolean) => setDraft(checked ? [...options] : []);

  const commit = () => {
    onChange(draft);
    setOpen(false);
  };
  const discard = () => setOpen(false);

  return (
    <Popover open={open} onOpenChange={openWith}>
      <PopoverTrigger asChild>
        <button type="button" className="flex min-h-8 flex-wrap items-center gap-1 text-left">
          {value.length === 0 ? (
            <span className="text-sm italic text-muted-foreground underline decoration-dashed underline-offset-4">
              Add permissions
            </span>
          ) : (
            <>
              {value.map((v) => (
                <Badge key={v} variant="secondary">
                  {v}
                </Badge>
              ))}
              <Pencil className="h-3 w-3 text-muted-foreground" />
            </>
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-56 space-y-3" align="start">
        <div className="text-sm font-medium">Permissions</div>
        <div className="space-y-2">
          {options.map((opt) => (
            <div key={opt} className="flex items-center gap-2">
              <Checkbox
                id={`perm-${opt}`}
                aria-label={opt}
                checked={draft.includes(opt)}
                onCheckedChange={(c) => toggle(opt, c === true)}
              />
              <Label htmlFor={`perm-${opt}`} className="font-normal">
                {opt}
              </Label>
            </div>
          ))}
          {options.length > 1 && (
            <div className="flex items-center gap-2 border-t pt-2">
              <Checkbox
                id="perm-select-all"
                aria-label="Select all"
                checked={allSelected}
                onCheckedChange={(c) => toggleAll(c === true)}
              />
              <Label htmlFor="perm-select-all" className="font-normal">
                Select all
              </Label>
            </div>
          )}
        </div>
        <div className="flex items-center justify-end gap-2">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label="Discard"
            className={cn("text-destructive hover:text-destructive")}
            onClick={discard}
          >
            <X className="h-4 w-4" />
          </Button>
          <Button type="button" size="icon" aria-label="Confirm" onClick={commit}>
            <Check className="h-4 w-4" />
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
