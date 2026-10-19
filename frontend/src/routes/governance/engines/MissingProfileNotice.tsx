import { useLocation } from "react-router-dom";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { MISSING_PROFILE_STATE } from "@/routes/governance/engines/PolicyRedirect";

/**
 * The index's notice after an old Policies URL named a profile that does not
 * exist (spec §3): `PolicyRedirect` lands here with the name in location state.
 */
export function MissingProfileNotice() {
  const state: unknown = useLocation().state;
  const name =
    state && typeof state === "object" && MISSING_PROFILE_STATE in state
      ? String((state as Record<string, unknown>)[MISSING_PROFILE_STATE])
      : null;
  if (name === null) return null;
  return (
    <Alert role="status">
      <AlertDescription>{`No policy profile named ${name}.`}</AlertDescription>
    </Alert>
  );
}
