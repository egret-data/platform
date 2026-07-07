import { Settings2 } from "lucide-react";
import { type ResourceLevel, resourceLookup } from "@/api/policies";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { TagCombobox } from "@/components/ui/tag-combobox";

interface Props {
  label: string;
  level: string;
  service: string;
  parents: { catalog?: string; schema?: string; table?: string };
  value: ResourceLevel;
  onChange: (v: ResourceLevel) => void;
  /** Whether the service-def marks this resource level as supporting Exclude. */
  excludesSupported?: boolean;
  /** Whether the service-def marks this resource level as supporting Recursive. */
  recursiveSupported?: boolean;
  /** Whether the service-def marks this resource level as mandatory. */
  mandatory?: boolean;
  /** True when `mandatory` and no value has been provided yet. */
  invalid?: boolean;
}

export function ResourceLevelEditor({
  label,
  level,
  service,
  parents,
  value,
  onChange,
  excludesSupported = false,
  recursiveSupported = false,
  mandatory = false,
  invalid = false,
}: Props) {
  const showAdvanced = excludesSupported || recursiveSupported;

  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between gap-2">
        <label className="text-sm font-medium">{label}</label>
        {showAdvanced && (
          <Popover>
            <PopoverTrigger asChild>
              <button type="button" aria-label={`Advanced ${label}`}
                className={`inline-flex h-6 items-center gap-1 rounded px-1.5 text-xs ${
                  value.isExcludes || value.isRecursive ? "text-primary" : "text-muted-foreground"
                } hover:bg-accent`}>
                <Settings2 className="h-3.5 w-3.5" />
                {(value.isExcludes || value.isRecursive) && <span className="h-1.5 w-1.5 rounded-full bg-primary" />}
              </button>
            </PopoverTrigger>
            <PopoverContent align="end" className="w-56 space-y-3 p-3" onOpenAutoFocus={(e) => e.preventDefault()}>
              {excludesSupported && (
                <div className="flex items-center justify-between">
                  <div className="text-sm">
                    <div className="font-medium">Exclude</div>
                    <div className="text-xs text-muted-foreground">Match everything except these values</div>
                  </div>
                  <Switch aria-label={`Exclude ${label}`} checked={!!value.isExcludes}
                    onCheckedChange={(c) => onChange({ ...value, isExcludes: c })} />
                </div>
              )}
              {recursiveSupported && (
                <div className="flex items-center justify-between">
                  <div className="text-sm">
                    <div className="font-medium">Recursive</div>
                    <div className="text-xs text-muted-foreground">Apply to child resources</div>
                  </div>
                  <Switch aria-label={`Recursive ${label}`} checked={!!value.isRecursive}
                    onCheckedChange={(c) => onChange({ ...value, isRecursive: c })} />
                </div>
              )}
            </PopoverContent>
          </Popover>
        )}
      </div>
      <div className={mandatory && invalid ? "rounded-md ring-1 ring-destructive" : undefined}>
        <TagCombobox
          values={value.values}
          onChange={(vals) => onChange({ ...value, values: vals })}
          suggest={(q) => resourceLookup(service, level, parents, q).then((vs) => vs.map((v) => ({ value: v })))}
          queryKey={["resource-lookup", service, level, parents]}
          placeholder="value or * (Enter to add)"
          creatable
          ariaLabel={`${label} value`}
        />
      </div>
      {mandatory && invalid && <p className="text-xs text-destructive">Required</p>}
    </div>
  );
}
