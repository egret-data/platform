import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { X } from "lucide-react";
import { type Principal, listPrincipals } from "@/api/policies";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

interface Props {
  users: string[];
  groups: string[];
  onChange: (next: { users: string[]; groups: string[] }) => void;
}

export function PrincipalPicker({ users, groups, onChange }: Props) {
  const [q, setQ] = useState("");
  const [open, setOpen] = useState(false);

  const suggestions = useQuery<Principal[]>({
    queryKey: ["principals", q],
    queryFn: () => listPrincipals(q),
    enabled: open,
    retry: false,
  });

  const add = (p: Principal) => {
    if (p.kind === "user") {
      if (!users.includes(p.name)) onChange({ users: [...users, p.name], groups });
    } else if (!groups.includes(p.name)) {
      onChange({ users, groups: [...groups, p.name] });
    }
    setQ("");
  };

  const removeUser = (n: string) => onChange({ users: users.filter((u) => u !== n), groups });
  const removeGroup = (n: string) => onChange({ users, groups: groups.filter((g) => g !== n) });

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap gap-1">
        {users.map((u) => (
          <Badge key={`u:${u}`} variant="secondary" className="gap-1">
            {u}
            <button type="button" aria-label={`remove ${u}`} onClick={() => removeUser(u)}>
              <X className="h-3 w-3" />
            </button>
          </Badge>
        ))}
        {groups.map((g) => (
          <Badge key={`g:${g}`} variant="outline" className="gap-1">
            {g}
            <button type="button" aria-label={`remove ${g}`} onClick={() => removeGroup(g)}>
              <X className="h-3 w-3" />
            </button>
          </Badge>
        ))}
      </div>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Input
            placeholder="Add user or group…"
            value={q}
            onFocus={() => setOpen(true)}
            onChange={(e) => setQ(e.target.value)}
          />
        </PopoverTrigger>
        <PopoverContent align="start" className="max-h-64 w-64 overflow-y-auto p-1">
          {(suggestions.data ?? []).map((p) => (
            <button
              key={`${p.kind}:${p.name}`}
              type="button"
              onClick={() => add(p)}
              className="flex w-full items-center justify-between rounded px-2 py-1.5 text-sm hover:bg-accent"
            >
              <span>{p.label}</span>
              <span className="text-xs text-muted-foreground">{p.kind}</span>
            </button>
          ))}
          {suggestions.data?.length === 0 && (
            <div className="px-2 py-1.5 text-sm text-muted-foreground">No matches.</div>
          )}
        </PopoverContent>
      </Popover>
    </div>
  );
}
