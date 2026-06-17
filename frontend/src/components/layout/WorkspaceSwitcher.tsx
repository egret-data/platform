import { useNavigate, useParams } from "react-router-dom";
import { useMyWorkspaces } from "@/api/hooks";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

export function WorkspaceSwitcher() {
  const navigate = useNavigate();
  const { workspace } = useParams();
  const { data: workspaces } = useMyWorkspaces(true);

  return (
    <Select value={workspace ?? ""} onValueChange={(slug) => navigate(`/explore/${slug}`)}>
      <SelectTrigger className="w-56" aria-label="Workspace">
        <SelectValue placeholder="Select a workspace" />
      </SelectTrigger>
      <SelectContent>
        {(workspaces ?? []).map((w) => (
          <SelectItem key={w.id} value={w.slug}>{w.name}</SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
