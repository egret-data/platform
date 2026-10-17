import { Link, useParams } from "react-router-dom";
import { PEOPLE_BASE, principalParam } from "@/routes/governance/people/peopleUrl";

export type PrincipalKindOfPage = "user" | "group" | "role";

const KIND_LABEL: Record<PrincipalKindOfPage, string> = { user: "User", group: "Group", role: "Role" };

/**
 * A user's, group's or role's page. This is the frame only: the header and
 * tabs of spec §2 are built on it (plan Task 7).
 */
export default function PrincipalPage({ kind }: { kind: PrincipalKindOfPage }) {
  const params = useParams();
  const key = principalParam(kind === "user" ? params.userId : params.name);
  return (
    <div className="space-y-1 p-4">
      <Link to={PEOPLE_BASE} className="text-sm text-muted-foreground hover:underline">
        People & groups
      </Link>
      <p className="text-xs uppercase text-muted-foreground">{KIND_LABEL[kind]}</p>
      <h1 className="break-all text-lg font-semibold">{key}</h1>
    </div>
  );
}
