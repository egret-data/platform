import { useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useSchedule, type ScheduleType } from "@/api/alerts";
import {
  FORM_ID,
  SECTIONS,
  ScheduleEditor,
  type SectionKey,
} from "@/components/alerts/ScheduleEditor";
import { Button } from "@/components/ui/button";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";

/**
 * The create/edit page for one schedule.
 *
 * A PAGE, not a dialog, because that is what every sibling does: a chart, a
 * dashboard, a dataset and a notebook each have their own route at `/:id`, and
 * a dialog — where one is used at all — only asks for a name. Alerts were the
 * exception, and after the depth pass the form carries roughly twenty fields,
 * which no modal reads well.
 *
 * Being a route brings one obligation a modal did not have: someone can
 * navigate away mid-edit, so the unsaved-changes guard is armed here.
 */
export default function ScheduleDetail() {
  const { workspace: ws = "", id } = useParams();
  const nav = useNavigate();
  const [params, setParams] = useSearchParams();
  const isNew = !id || id === "new";
  const listHref = `/explore/${ws}/alerts`;

  const { data: existing, isLoading, isError, error } = useSchedule(ws, isNew ? null : (id ?? null));

  // A new schedule takes its type from the query (`?type=report`, which is what
  // the list's own tab links to); an existing one is whatever it was saved as,
  // since the type is immutable server-side.
  const type: ScheduleType = isNew
    ? params.get("type") === "report"
      ? "report"
      : "alert"
    : ((existing?.type as ScheduleType) ?? "alert");

  const prefillKind = params.get("kind");
  const prefill: { kind: "chart" | "dashboard"; object: string } | undefined =
    isNew && (prefillKind === "chart" || prefillKind === "dashboard")
      ? { kind: prefillKind, object: params.get("object") ?? "" }
      : undefined;

  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  // The open section rides in the URL, like DatasetDetail's `?tab` — so a
  // colleague can be sent straight to the part being discussed, and Back walks
  // out of the form the way it walked in.
  const sectionParam = params.get("section");
  const section: SectionKey = SECTIONS.some((s) => s.key === sectionParam)
    ? (sectionParam as SectionKey)
    : "general";
  const setSection = (key: SectionKey) =>
    setParams(
      (p) => {
        if (key === "general") p.delete("section");
        else p.set("section", key);
        return p;
      },
      { replace: true },
    );
  useUnsavedChangesWarning(dirty);

  const backHref = type === "report" ? `${listHref}?type=report` : listHref;
  const done = () => {
    setDirty(false);
    nav(backHref);
  };

  const heading = isNew
    ? `New ${type === "report" ? "report" : "alert"}`
    : (existing?.name ?? "…");

  return (
    <div className="mx-auto max-w-6xl p-4">
      <Link className="text-sm text-muted-foreground hover:underline" to={backHref}>
        ← Alerts &amp; Reports
      </Link>
      <h1 className="mb-4 mt-1 truncate text-lg font-semibold">{heading}</h1>

      {isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {(error as Error)?.message ?? "Could not load this schedule."}
          </AlertDescription>
        </Alert>
      ) : !isNew && isLoading ? (
        <ListSkeleton rows={6} cols={2} />
      ) : (
        <div className="grid grid-cols-1 gap-6 lg:grid-cols-[13rem_minmax(0,1fr)]">
          {/* The settings-hub shape this app already uses (WorkspaceSettings):
              a vertical section nav that turns a long scroll into five short
              panels, and puts the spare width to work instead of leaving it
              blank beside a narrow column. */}
          <nav
            aria-label="Sections"
            className="flex gap-1 overflow-x-auto lg:sticky lg:top-0 lg:flex-col lg:self-start lg:overflow-visible"
          >
            {SECTIONS.map((s) => {
              const active = s.key === section;
              return (
                <button
                  key={s.key}
                  type="button"
                  aria-current={active ? "page" : undefined}
                  onClick={() => setSection(s.key)}
                  className={`shrink-0 rounded-md px-3 py-2 text-left text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${
                    active
                      ? "bg-primary/10 font-medium text-primary"
                      : "text-muted-foreground hover:bg-accent hover:text-foreground"
                  }`}
                >
                  {s.label(type === "alert")}
                </button>
              );
            })}
          </nav>

          <div className="min-w-0">
            <ScheduleEditor
              ws={ws}
              type={type}
              scheduleId={isNew ? null : (id ?? null)}
              prefill={prefill}
              section={section}
              onSectionChange={setSection}
              onDirtyChange={setDirty}
              onBusyChange={setBusy}
              onDone={done}
            />
          </div>
        </div>
      )}

      {/* NOT sticky. `sticky bottom-0` pins to the scrollport edge, which sits
          above this layout's own bottom padding, so a band of the form stays
          visible under the bar however the margins are tuned — tried three
          ways. A plain row at the end of the page has no such failure, and it
          is what DatasetDetail does. */}
      {!isError && (
        <div className="mt-4 border-t pt-3">
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={done}>
              Cancel
            </Button>
            <Button type="submit" form={FORM_ID} disabled={busy}>
              {isNew ? "Create" : "Save"}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
