import { Navigate, Route, Routes } from "react-router-dom";
import ByNameRedirect from "@/routes/governance/people/ByNameRedirect";
import PeopleIndex from "@/routes/governance/people/PeopleIndex";
import PrincipalPage from "@/routes/governance/people/PrincipalPage";

/**
 * The People area's routes (spec P1), mounted lazily by `App.tsx` at
 * `/governance/people/*` behind `policy:read` (FE-ROUTE-2, P2). `by-name` is a
 * static segment, so it ranks above `users/:userId`. A URL naming nothing goes
 * back to the index, by an absolute path (FE-ROUTE-1).
 */
export default function PeopleRoutes() {
  return (
    <Routes>
      <Route index element={<PeopleIndex />} />
      <Route path="users/by-name/:username" element={<ByNameRedirect />} />
      <Route path="users/:userId" element={<PrincipalPage kind="user" />} />
      <Route path="groups/:name" element={<PrincipalPage kind="group" />} />
      <Route path="roles/:name" element={<PrincipalPage kind="role" />} />
      <Route path="*" element={<Navigate to="/governance/people" replace />} />
    </Routes>
  );
}
