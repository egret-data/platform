import { Plus, Trash2 } from "lucide-react";
import { type AccessItem, listPrincipals } from "@/api/policies";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { TagCombobox } from "@/components/ui/tag-combobox";
import { PermissionPickerCell } from "@/components/policy/PermissionPickerCell";

export interface PermissionTableProps {
  items: AccessItem[];
  onChange: (items: AccessItem[]) => void;
  /** Permission options for the Permissions popover. */
  accessTypes: string[];
  /** Effect applied to rows added via "Add row". Defaults to "allow". */
  effect?: "allow" | "deny";
}

const blankItem = (effect: "allow" | "deny"): AccessItem => ({
  users: [],
  groups: [],
  roles: [],
  accesses: [],
  effect,
  delegateAdmin: false,
});

/**
 * Ranger-style permission table: one row per policy item, columns
 * Roles | Groups | Users | Permissions | Delegate admin | (remove).
 */
export function PermissionTable({
  items,
  onChange,
  accessTypes,
  effect = "allow",
}: PermissionTableProps) {
  const setItem = (i: number, next: Partial<AccessItem>) =>
    onChange(items.map((it, j) => (j === i ? { ...it, ...next } : it)));
  const addRow = () => onChange([...items, blankItem(effect)]);
  const removeRow = (i: number) => onChange(items.filter((_, j) => j !== i));

  return (
    <div className="space-y-2">
      <div className="overflow-x-auto rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Roles</TableHead>
              <TableHead>Groups</TableHead>
              <TableHead>Users</TableHead>
              <TableHead>Permissions</TableHead>
              <TableHead>Delegate admin</TableHead>
              <TableHead />
            </TableRow>
          </TableHeader>
          <TableBody>
            {items.map((it, i) => (
              <TableRow key={i}>
                <TableCell>
                  <TagCombobox
                    values={it.roles}
                    onChange={(v) => setItem(i, { roles: v })}
                    suggest={async () => []}
                    queryKey={["roles"]}
                    placeholder="role"
                    creatable
                    ariaLabel={`role ${i}`}
                  />
                </TableCell>
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
                  <PermissionPickerCell
                    value={it.accesses}
                    options={accessTypes}
                    onChange={(acc) => setItem(i, { accesses: acc })}
                  />
                </TableCell>
                <TableCell>
                  <Checkbox
                    aria-label={`delegate admin ${i}`}
                    checked={!!it.delegateAdmin}
                    onCheckedChange={(c) => setItem(i, { delegateAdmin: c === true })}
                  />
                </TableCell>
                <TableCell>
                  <Button
                    type="button"
                    variant="ghost"
                    size="iconSm"
                    aria-label={`remove row ${i}`}
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
