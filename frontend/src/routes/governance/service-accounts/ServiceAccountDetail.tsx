import { Link, useParams } from "react-router-dom";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { SERVICE_ACCOUNTS_BASE } from "@/routes/governance/service-accounts/serviceAccountsUrl";

/** Placeholder: Task 3 builds the identity and membership, Task 4 the credentials. */
export default function ServiceAccountDetail() {
  const { id = "" } = useParams();
  return (
    <ListPageShell
      title="Service account"
      description={id}
      back={<Link to={SERVICE_ACCOUNTS_BASE}>Service accounts</Link>}
    >
      <p className="text-sm text-muted-foreground">Details are on the way.</p>
    </ListPageShell>
  );
}
