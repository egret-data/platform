import { Navigate, Route, Routes, useParams } from "react-router-dom";
import { MissingProfileNotice } from "@/routes/governance/engines/MissingProfileNotice";
import EnginePage from "@/routes/governance/engines/EnginePage";
import ProfilePage from "@/routes/governance/engines/ProfilePage";
import { ENGINES_BASE, profileParam } from "@/routes/governance/engines/engineUrl";

/**
 * The Engine & policy area's routes (spec §2), mounted lazily by `App.tsx` at
 * `/governance/engines/*` behind `policy:read` (FE-ROUTE-2, EP3). `profiles`
 * is a static segment, so it ranks above `:id`. The editor URLs keep the
 * policy kind (Q2). A URL naming nothing goes back to the index, by an
 * absolute path (FE-ROUTE-1).
 *
 * The index and the editor are placeholders until their tasks land (plan Tasks 6, 14).
 */
function IndexStub() {
  return (
    <div className="space-y-4">
      <h1 className="text-2xl font-semibold">Engine & policy</h1>
      <MissingProfileNotice />
    </div>
  );
}

function EditorStub({ under }: { under: "engine" | "profile" }) {
  const { id = "", service, kind = "", policyId } = useParams();
  const subject = under === "engine" ? `engine ${id}` : `profile ${profileParam(service)}`;
  return (
    <div data-testid="editor-stub">
      <h1 className="text-2xl font-semibold">Policy editor</h1>
      <p className="text-sm">{`${subject}, ${kind}, ${policyId ?? "new"}`}</p>
    </div>
  );
}

export default function EnginesRoutes() {
  return (
    <Routes>
      <Route index element={<IndexStub />} />
      <Route path="profiles" element={<Navigate to={ENGINES_BASE} replace />} />
      <Route path="profiles/:service" element={<ProfilePage />} />
      <Route path="profiles/:service/advanced/policies/:kind/new" element={<EditorStub under="profile" />} />
      <Route path="profiles/:service/advanced/policies/:kind/:policyId" element={<EditorStub under="profile" />} />
      <Route path=":id" element={<EnginePage />} />
      <Route path=":id/advanced/policies/:kind/new" element={<EditorStub under="engine" />} />
      <Route path=":id/advanced/policies/:kind/:policyId" element={<EditorStub under="engine" />} />
      <Route path="*" element={<Navigate to={ENGINES_BASE} replace />} />
    </Routes>
  );
}
