import { useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import {
  Activity,
  Bell,
  CalendarClock,
  FileBarChart,
  Filter,
  Globe,
  Send,
  Settings2,
} from "lucide-react";
import { useSchedule, type ScheduleType } from "@/api/alerts";
import {
  FORM_ID,
  SECTIONS,
  ScheduleEditor,
  type SectionKey,
} from "@/components/alerts/ScheduleEditor";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { DetailFact } from "@/components/layout/DetailFact";
import { RunLogTable } from "@/components/alerts/RunLogTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { StateBadge } from "./Alerts";
import { humanizeCrontab } from "@/lib/humanizeCron";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { PageContainer } from "@/components/layout/PageContainer";
import { DetailHeader } from "@/components/layout/DetailHeader";
import { StatusBadge } from "@/components/ui/status-badge";

/** The icon each section carries in the tab bar, in `SECTIONS` order. */
const SECTION_ICONS: Record<SectionKey, typeof Bell> = {
  general: Settings2,
  what: Filter,
  schedule: CalendarClock,
  notification: Send,
};

/**
 * The create/edit page for one schedule.
 *
 * A PAGE, not a dialog, because that is what every sibling does: a chart, a
 * dashboard, a dataset and a notebook each have their own route at `/:id`, and
 * a dialog — where one is used at all — only asks for a name. Alerts were the
 * exception, and after the depth pass the form carries roughly twenty fields,
 * which no modal reads well.
 *
 * The frame is `WarehouseDetail`'s: an icon tile, the name, a state badge, a
 * strip of facts, then horizontal tabs. That page is where this app decided
 * what a detail page looks like, and a schedule is the same kind of thing — an
 * object with a status and several panels of configuration. The facts are the
 * three questions a reader has before editing anything: is it on, when does it
 * run, and how did it go last time.
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

  // A new schedule takes its type from the query (`?type=report` /
  // `?type=dataapi_alert`, which is what the list's own entry points link
  // to); an existing one is whatever it was saved as, since the type is
  // immutable server-side.
  const type: ScheduleType = isNew
    ? params.get("type") === "report"
      ? "report"
      : params.get("type") === "dataapi_alert"
        ? "dataapi_alert"
        : "alert"
    : ((existing?.type as ScheduleType) ?? "alert");
  const isAlert = type === "alert";
  const isDataapiAlert = type === "dataapi_alert";

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
    ? `New ${type === "report" ? "report" : isDataapiAlert ? "Data API alert" : "alert"}`
    : (existing?.name ?? "…");
  // `Activity` — the same icon the sidebar's Data API → Activity item uses —
  // rather than reusing the report icon for a type that has neither a report
  // format nor a rendered attachment.
  const Icon = isAlert ? Bell : isDataapiAlert ? Activity : FileBarChart;

  return (
    // A form editor: the 960px column on the list's left edge (FE-PAGE-6). With
    // the section nav horizontal there is no column beside the form to fill,
    // and a full-width single-column form is past a readable measure.
    <PageContainer width="narrow">
      <DetailHeader
        back={{ to: backHref, label: "Alerts & Reports" }}
        /* The title wraps (DetailHeader's default): truncated, "New Data API
           alert" read "New Data API a…" at 390px (KI-233). */
        title={heading}
        /* Paused is the state that explains "why has this not fired?", so it
           earns the badge over the run state; an active schedule shows how its
           last run went instead, which is the other half of that question. A
           schedule that does not exist yet has neither. */
        status={
          !isNew && existing ? (
            existing.active ? (
              <StateBadge state={existing.last_state} />
            ) : (
              <StatusBadge tone="inactive">Paused</StatusBadge>
            )
          ) : undefined
        }
        meta={
          !isNew && existing ? (
            <>
              <DetailFact
                icon={Icon}
                label="Type"
                value={isAlert ? "Alert" : isDataapiAlert ? "Data API alert" : "Report"}
                to={backHref}
              />
              <DetailFact
                icon={CalendarClock}
                label="Runs"
                value={humanizeCrontab(existing.crontab)}
                title={existing.crontab}
              />
              <DetailFact icon={Globe} label="Timezone" value={existing.timezone} />
            </>
          ) : undefined
        }
      />

      {isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {(error as Error)?.message ?? "Could not load this schedule."}
          </AlertDescription>
        </Alert>
      ) : !isNew && isLoading ? (
        <ListSkeleton rows={6} cols={2} />
      ) : (
        // Horizontal tabs, the shape `WarehouseDetail` uses. One `TabsContent`
        // holding the whole form rather than one per section: the editor is a
        // SINGLE form element whose state lives in it, and splitting it across
        // five panels would unmount the fields on every tab change. The test
        // "keeps what you typed when you move between sections" is what pins
        // that, and it fails if this is split.
        <Tabs value={section} onValueChange={(v) => setSection(v as SectionKey)}>
          <TabsList>
            {SECTIONS.map((s) => {
              const SIcon = SECTION_ICONS[s.key];
              return (
                <TabsTrigger key={s.key} value={s.key}>
                  <SIcon className="h-4 w-4" />
                  {s.label(type)}
                </TabsTrigger>
              );
            })}
          </TabsList>
          <TabsContent value={section} className="mt-4">
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
          </TabsContent>
        </Tabs>
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

      {/* Below the actions, because it is not part of the form: the rows above
          are what you CHANGE, these are what happened. It answers the question
          the header's badge raises — an "Error" chip with no error on the same
          screen sends you to the list to open a dialog for it — and it is why
          a short section like General no longer ends in half a screen of
          white. A slice, not the log: the drawer on the list is still where
          the whole history lives. */}
      {!isNew && !isError && id && (
        <div className="mt-6">
          <h2 className="text-sm font-medium">Recent runs</h2>
          <p className="mb-2 text-xs text-muted-foreground">
            The last five, newest first.
          </p>
          <div className="rounded-lg border px-4 py-1">
            <RunLogTable ws={ws} scheduleId={id} limit={5} />
          </div>
        </div>
      )}
    </PageContainer>
  );
}
