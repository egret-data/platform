import { useEffect, useId, useRef, useState } from "react";
import { Outlet, useLocation, useParams, useSearchParams } from "react-router-dom";
import { Database, PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { EmptyState } from "@/components/explore/EmptyState";
import { Button } from "@/components/ui/button";
import { CatalogTree } from "@/routes/governance/data/CatalogTree";
import { urlToPath } from "@/routes/governance/data/objectPath";

/**
 * The Data area's frame (spec §2.2, Unity Catalog's Catalog Explorer): the
 * catalog tree on the left, 18rem wide, and the selected object's page beside
 * it (`<Outlet/>`).
 *
 * Below 768 px (`md`) the two cannot sit side by side, so the tree folds away
 * under a "Show catalogs" toggle and closes again when the user opens an
 * object from it. At `md` and up it is always shown and the toggle is gone.
 *
 * Folding the tree hides the row that held focus, which would drop focus to
 * `<body>` (D7); so the opened page's heading takes it (FE-A11Y: focus is
 * never lost).
 */
export default function DataPage() {
  // A layout route sees its child route's params: react-router hands every
  // match of one branch the same params object.
  const params = useParams();
  const [search] = useSearchParams();
  const selected = params.catalog === undefined ? null : urlToPath(params, search);

  // Opening an object closes the folded tree; adjusted while rendering, not in an effect.
  const [treeOpen, setTreeOpen] = useState(false);
  const { pathname } = useLocation();
  const [treeOpenedAt, setTreeOpenedAt] = useState(pathname);
  // Counts the folds that opened a page, so each one moves focus once.
  const [folds, setFolds] = useState(0);
  if (treeOpenedAt !== pathname) {
    setTreeOpenedAt(pathname);
    if (treeOpen) {
      setTreeOpen(false);
      setFolds((n) => n + 1);
    }
  }
  const pageRef = useRef<HTMLElement>(null);
  useEffect(() => {
    if (folds === 0) return;
    // At `md` and up the tree stays shown, and focus stays in it.
    if (typeof window.matchMedia === "function" && window.matchMedia("(min-width: 768px)").matches) return;
    pageRef.current?.querySelector<HTMLElement>("h1")?.focus();
  }, [folds]);
  const treeId = useId();
  const Toggle = treeOpen ? PanelLeftClose : PanelLeftOpen;

  return (
    <div className="flex min-h-full flex-col gap-4 md:grid md:grid-cols-[18rem_minmax(0,1fr)] md:gap-6">
      <div className="md:hidden">
        <Button
          variant="outline"
          size="sm"
          aria-expanded={treeOpen}
          aria-controls={treeId}
          onClick={() => setTreeOpen((o) => !o)}
        >
          <Toggle aria-hidden />
          {treeOpen ? "Hide catalogs" : "Show catalogs"}
        </Button>
      </div>
      <aside
        id={treeId}
        aria-label="Catalog tree"
        className={`${treeOpen ? "block" : "hidden"} min-w-0 md:block md:border-r md:border-border md:pr-4`}
      >
        <CatalogTree selected={selected} />
      </aside>
      <section ref={pageRef} className="min-w-0">
        <Outlet />
      </section>
    </div>
  );
}

/** `/governance/data` with nothing selected (spec §2.1). */
export function DataIndex() {
  return (
    <div className="space-y-4">
      <h1 tabIndex={-1} className="text-2xl font-semibold outline-none">
        Data
      </h1>
      <EmptyState
        icon={Database}
        title="Pick a catalog"
        description="Open a catalog, schema or table from the tree to see who can read it, its tags and its rules. Go to path opens a known catalog.schema.table directly."
      />
    </div>
  );
}
