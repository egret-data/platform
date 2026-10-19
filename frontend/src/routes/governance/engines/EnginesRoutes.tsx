import { Navigate, Route, Routes } from "react-router-dom";
import EnginesIndex from "@/routes/governance/engines/EnginesIndex";
import EnginePage from "@/routes/governance/engines/EnginePage";
import ProfilePage from "@/routes/governance/engines/ProfilePage";
import { EngineEditorRoute, ProfileEditorRoute } from "@/routes/governance/engines/EditorRoute";
import { ENGINES_BASE } from "@/routes/governance/engines/engineUrl";

/**
 * The Engine & policy area's routes (spec §2), mounted lazily by `App.tsx` at
 * `/governance/engines/*` behind `policy:read` (FE-ROUTE-2, EP3). `profiles`
 * is a static segment, so it ranks above `:id`. The editor URLs keep the
 * policy kind (Q2). A URL naming nothing goes back to the index, by an
 * absolute path (FE-ROUTE-1).
 */
export default function EnginesRoutes() {
  return (
    <Routes>
      <Route index element={<EnginesIndex />} />
      <Route path="profiles" element={<Navigate to={ENGINES_BASE} replace />} />
      <Route path="profiles/:service" element={<ProfilePage />} />
      <Route path="profiles/:service/advanced/policies/:kind/new" element={<ProfileEditorRoute />} />
      <Route path="profiles/:service/advanced/policies/:kind/:policyId" element={<ProfileEditorRoute />} />
      <Route path=":id" element={<EnginePage />} />
      <Route path=":id/advanced/policies/:kind/new" element={<EngineEditorRoute />} />
      <Route path=":id/advanced/policies/:kind/:policyId" element={<EngineEditorRoute />} />
      <Route path="*" element={<Navigate to={ENGINES_BASE} replace />} />
    </Routes>
  );
}
