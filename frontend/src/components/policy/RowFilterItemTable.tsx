import { Plus, Trash2 } from "lucide-react";
import { type RowFilterItem, listPrincipals } from "@/api/policies";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { TagCombobox } from "@/components/ui/tag-combobox";

export interface RowFilterItemTableProps {
  items: RowFilterItem[];
  onChange: (items: RowFilterItem[]) => void;
}

const blankItem = (): RowFilterItem => ({ users: [], groups: [], filterExpr: "" });

/**
 * Table form for row-filter policy items — Groups | Users | Filter expression
 * | (remove), mirroring `PermissionTable`'s row shape so row filters read
 * consistently with the access editor instead of the old one-card-per-item
 * layout.
 */
export function RowFilterItemTable({ items, onChange }: RowFilterItemTableProps) {
  const setItem = (i: number, next: Partial<RowFilterItem>) =>
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
              <TableHead>Filter expression</TableHead>
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
                  <Input
                    aria-label={`filter ${i}`}
                    placeholder="e.g. country = 'US'"
                    value={it.filterExpr}
                    onChange={(e) => setItem(i, { filterExpr: e.target.value })}
                  />
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
