import { Navigate, Route, Routes } from "react-router-dom";
import ClassificationsIndex from "@/routes/governance/classifications/ClassificationsIndex";
import TagPage from "@/routes/governance/classifications/TagPage";
import { CLASSIFICATIONS_BASE } from "@/routes/governance/classifications/tagUrl";

/**
 * The Classification area's routes (spec §2), mounted lazily by `App.tsx` at
 * `/governance/classifications/*` behind `tag:read` (FE-ROUTE-2). The tag
 * page's tab is `?tab=` (CL17). A URL naming nothing else goes back to the
 * index, by an absolute path (FE-ROUTE-1).
 */
export default function ClassificationsRoutes() {
  return (
    <Routes>
      <Route index element={<ClassificationsIndex />} />
      <Route path=":id" element={<TagPage />} />
      <Route path="*" element={<Navigate to={CLASSIFICATIONS_BASE} replace />} />
    </Routes>
  );
}
