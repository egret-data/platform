import { listPrincipals } from "@/api/policies";
import { TagCombobox } from "@/components/ui/tag-combobox";

interface Props {
  users: string[];
  groups: string[];
  onChange: (next: { users: string[]; groups: string[] }) => void;
}

export function PrincipalPicker({ users, groups, onChange }: Props) {
  return (
    <div className="space-y-3">
      <div className="space-y-1">
        <div className="text-xs font-medium text-muted-foreground">Users</div>
        <TagCombobox
          values={users}
          onChange={(v) => onChange({ users: v, groups })}
          suggest={(q) =>
            listPrincipals(q, "user").then((ps) => ps.map((p) => ({ value: p.name, label: p.label })))
          }
          queryKey={["principals", "user"]}
          placeholder="Add user…"
          ariaLabel="Add user"
          creatable
        />
      </div>
      <div className="space-y-1">
        <div className="text-xs font-medium text-muted-foreground">Groups</div>
        <TagCombobox
          values={groups}
          onChange={(v) => onChange({ users, groups: v })}
          suggest={(q) =>
            listPrincipals(q, "group").then((ps) => ps.map((p) => ({ value: p.name, label: p.label })))
          }
          queryKey={["principals", "group"]}
          placeholder="Add group…"
          ariaLabel="Add group"
          creatable
        />
      </div>
    </div>
  );
}
