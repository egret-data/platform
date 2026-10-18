import { SubjectExplainForm, type Subject } from "@/components/perm/SubjectExplain";
import type { Principal } from "@/routes/governance/people/principalData";

/** Spec §2: what a user with no username's Check access says; nothing is asked. */
export const CHECK_NEEDS_USERNAME = "No username yet, so no engine knows this user.";

/**
 * A principal's Check access tab (spec §2): an action and a target
 * (`ScopePicker`), asked about this principal. A user is asked through
 * `POST /perm/explain` by engine username and answered per policy profile
 * (`ExplainResultView`); a group or a role through
 * `POST /perm/explain/principal`, each member answered on their own
 * (`MembersExplainView`), with no verdict for the group. A user with no
 * username is known to no engine, so nothing is asked. `public` has no such
 * tab (`TABS_BY_KIND`).
 */
export function CheckAccessTab({ principal }: { principal: Principal }) {
  let subject: Subject;
  if (principal.kind === "user") {
    const username = principal.data.username;
    if (!username) return <p className="text-sm">{CHECK_NEEDS_USERNAME}</p>;
    subject = { kind: "user", ref: username };
  } else {
    subject = { kind: principal.kind, ref: principal.data.name };
  }
  return (
    <SubjectExplainForm
      subject={subject}
      layout={({ action, target, submit }) => (
        <div className="grid gap-6 rounded-lg border border-border p-4 md:grid-cols-2 md:p-6">
          <div className="space-y-4">{action}</div>
          <div className="space-y-4">
            {target}
            <div className="flex flex-wrap items-center gap-2">{submit}</div>
          </div>
        </div>
      )}
    />
  );
}
