import { Navigate, Route, Routes } from "react-router-dom";
import DataPage, { DataIndex } from "@/routes/governance/data/DataPage";
import ObjectPage from "@/routes/governance/data/ObjectPage";

/**
 * The Data area's routes (spec §2.1), mounted lazily by `App.tsx` at
 * `/governance/data/*` (FE-ROUTE-2): the index, then one route per level, all
 * inside `DataPage`'s frame. A deeper URL names nothing and goes back to the
 * index, by an absolute path (FE-ROUTE-1).
 */
export default function DataRoutes() {
  return (
    <Routes>
      <Route element={<DataPage />}>
        <Route index element={<DataIndex />} />
        <Route path=":catalog" element={<ObjectPage />} />
        <Route path=":catalog/:schema" element={<ObjectPage />} />
        <Route path=":catalog/:schema/:object" element={<ObjectPage />} />
        <Route path="*" element={<Navigate to="/governance/data" replace />} />
      </Route>
    </Routes>
  );
}
