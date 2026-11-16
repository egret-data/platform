import { Navigate, Route, Routes } from "react-router-dom";
import ServiceAccountDetail from "@/routes/governance/service-accounts/ServiceAccountDetail";
import ServiceAccountsIndex from "@/routes/governance/service-accounts/ServiceAccountsIndex";

/**
 * The Service accounts area's routes (SA3), mounted lazily by `App.tsx` at
 * `/governance/service-accounts/*` behind `user:read` (FE-ROUTE-2). A URL
 * naming nothing goes back to the index, by an absolute path (FE-ROUTE-1).
 */
export default function ServiceAccountsRoutes() {
  return (
    <Routes>
      <Route index element={<ServiceAccountsIndex />} />
      <Route path=":id" element={<ServiceAccountDetail />} />
      <Route path="*" element={<Navigate to="/governance/service-accounts" replace />} />
    </Routes>
  );
}
