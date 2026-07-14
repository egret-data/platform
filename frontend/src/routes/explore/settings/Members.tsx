import { useParams } from "react-router-dom";

import { MembersPanel } from "@/components/access/MembersPanel";

export default function Members() {
  const ws = useParams().workspace!;
  return <MembersPanel workspaceId={ws} />;
}
