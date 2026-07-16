import { Plus, Trash2 } from "lucide-react";
import { type MaskItem, MASK_TYPES, listPrincipals } from "@/api/policies";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { TagCombobox } from "@/components/ui/tag-combobox";

export interface MaskItemTableProps {
  items: MaskItem[];
  onChange: (items: MaskItem[]) => void;
  /** Mask types from the profile's service-def; falls back to the Trino set. */
  maskTypes?: readonly { name: string; label?: string }[];
}

const blankItem = (): MaskItem => ({ users: [], groups: [], maskType: "MASK_HASH", valueExpr: undefined });

/**
 * Table form for masking policy items — Groups | Users | Mask type | (remove),
 * mirroring `PermissionTable`'s row shape so masking reads consistently with
 * the access editor instead of the old one-card-per-item layout.
 */
export function MaskItemTable({ items, onChange, maskTypes }: MaskItemTableProps) {
  const types = maskTypes && maskTypes.length ? maskTypes : MASK_TYPES;
  const setItem = (i: number, next: Partial<MaskItem>) =>
    onChange(items.map((it, j) => (j === i ? { ...it, ...next } : it)));
  const addRow = () => onChange([...items, blankItem()]);
  const removeRow = (i: number) => onChange(items.filter((_, j) => j !== i));

  return (
    <div className="space-y-2">
      <div className="overflow-x-auto rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Groups</TableHead>
              <TableHead>Users</TableHead>
              <TableHead>Mask type</TableHead>
              <TableHead />
            </TableRow>
          </TableHeader>
          <TableBody>
            {items.map((it, i) => (
              <TableRow key={i}>
                <TableCell>
                  <TagCombobox
                    values={it.groups}
                    onChange={(v) => setItem(i, { groups: v })}
                    suggest={(q) =>
                      listPrincipals(q, "group").then((ps) => ps.map((p) => ({ value: p.name, label: p.label })))
                    }
                    queryKey={["principals", "group"]}
                    placeholder="group"
                    ariaLabel={`group ${i}`}
                  />
                </TableCell>
                <TableCell>
                  <TagCombobox
                    values={it.users}
                    onChange={(v) => setItem(i, { users: v })}
                    suggest={(q) =>
                      listPrincipals(q, "user").then((ps) => ps.map((p) => ({ value: p.name, label: p.label })))
                    }
                    queryKey={["principals", "user"]}
                    placeholder="user"
                    ariaLabel={`user ${i}`}
                  />
                </TableCell>
                <TableCell>
                  <div className="space-y-2">
                    <Select value={it.maskType} onValueChange={(v) => setItem(i, { maskType: v })}>
                      <SelectTrigger aria-label={`mask type ${i}`} className="w-52">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {types.map((m) => (
                          <SelectItem key={m.name} value={m.name}>
                            {m.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    {it.maskType === "CUSTOM" && (
                      <Input
                        aria-label={`value expr ${i}`}
                        placeholder="cast(... {col} ...) as {type}"
                        value={it.valueExpr ?? ""}
                        onChange={(e) => setItem(i, { valueExpr: e.target.value })}
                      />
                    )}
                  </div>
                </TableCell>
                <TableCell>
                  <Button
                    type="button"
                    variant="ghost"
                    size="iconSm"
                    aria-label={`remove item ${i}`}
                    onClick={() => removeRow(i)}
                  >
                    <Trash2 className="h-3.5 w-3.5 text-muted-foreground" />
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
      <Button type="button" variant="outline" size="sm" onClick={addRow}>
        <Plus className="mr-1 h-4 w-4" /> Add row
      </Button>
    </div>
  );
}
