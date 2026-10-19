import { Navigate, Route, Routes } from "react-router-dom";
import { CLASSIFICATIONS_BASE } from "@/routes/governance/classifications/tagUrl";

/** Task 9 stub: replaced by the index in Task 10. */
function ClassificationsIndex() {
  return (
    <div>
      <h1 className="text-2xl font-semibold">Classifications</h1>
      <p className="text-sm text-muted-foreground">The tag and clearance catalogue.</p>
    </div>
  );
}

/** Task 9 stub: replaced by the tag page in Task 11. */
function TagPage() {
  return (
    <section aria-label="Classification">
      <h1 className="text-2xl font-semibold">Classification</h1>
    </section>
  );
}

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
