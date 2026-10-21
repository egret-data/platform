import { useId, useMemo, useRef, useState, type FormEvent, type ReactNode, type RefObject } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { MoreHorizontal, Plus, SearchX, Tags, Trash2 } from "lucide-react";
import { ApiError } from "@/api/client";
import { type Tag, useCreateTag, useTags } from "@/api/tags";
import { EmptyState } from "@/components/explore/EmptyState";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { TabError } from "@/routes/governance/tabParts";
import { DeleteTagDialog } from "@/routes/governance/classifications/DeleteTagDialog";
import { tagUrl } from "@/routes/governance/classifications/tagUrl";
import {
  ClearanceCell,
  dataCounts,
  DEFAULT_COLOR,
  slugifyTagKey,
  TagSwatch,
  useCanManageTags,
} from "@/routes/governance/classifications/tagParts";
import { TERM } from "@/routes/governance/glossary";

export const NO_CLASSIFICATION_YET = "No classification yet.";

/** Data and BI carriers: a tag carried by neither is unused. */
const isUnused = (t: Tag) => !(t.catalog_tables ?? 0) && !(t.catalog_columns ?? 0) && t.assignment_count === 0;

/** The words a row searches by (the shared table's own search is off: this page owns `?q=`). */
const haystack = (t: Tag) => `${t.name} ${t.key} ${t.description}`.toLowerCase();

/**
 * Governance → Classification (spec §3): the tag catalogue. One row per tag
 * with where it is carried (Data tables and columns, BI objects) and its
 * clearance; New classification for a `tag:manage` holder or platform admin.
 * The search is `?q=` (FE-PAGE-2), owned here so a miss can offer Clear
 * (FE-STATE-3).
 */
export default function ClassificationsIndex() {
  const canManage = useCanManageTags();
  const tagsQ = useTags();
  const [search, setSearch] = useSearchParams();
  const q = search.get("q") ?? "";
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState<Tag | null>(null);
  // The row's Actions button that opened the menu: the delete dialog returns focus to it.
  const menuOpener = useRef<HTMLButtonElement | null>(null);
  const createOpener = useRef<HTMLButtonElement | null>(null);
  const searchId = useId();

  const setQ = (next: string) =>
    setSearch(
      (prev) => {
        const params = new URLSearchParams(prev);
        if (next) params.set("q", next);
        else params.delete("q");
        return params;
      },
      { replace: true },
    );

  const rows = tagsQ.data ?? null;
  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return rows && needle ? rows.filter((t) => haystack(t).includes(needle)) : rows;
  }, [rows, q]);

  const newButton = canManage ? (
    <Button
      size="sm"
      onClick={(e) => {
        createOpener.current = e.currentTarget;
        setCreating(true);
      }}
    >
      <Plus className="mr-1 h-4 w-4" aria-hidden />
      New classification
    </Button>
  ) : null;

  const columns: Column<Tag>[] = [
    {
      key: "name",
      label: "Name",
      sortable: true,
      value: (t) => t.name,
      cell: (t) => (
        <span className="flex items-center gap-2">
          <TagSwatch color={t.color} />
          <Link to={tagUrl(t.id)} className="font-medium text-primary hover:underline">
            {t.name}
          </Link>
        </span>
      ),
    },
    {
      key: "key",
      label: "Key",
      sortable: true,
      value: (t) => t.key,
      cell: (t) => <span className="font-mono text-xs">{t.key}</span>,
    },
    {
      key: "description",
      label: "Description",
      cell: (t) => <span className="text-muted-foreground">{t.description || "—"}</span>,
    },
    {
      key: "data",
      label: "Data",
      sortable: true,
      value: (t) => (t.catalog_tables ?? 0) + (t.catalog_columns ?? 0),
      cell: (t) =>
        t.catalog_tables === undefined || t.catalog_columns === undefined ? (
          <span className="text-muted-foreground">Not reported</span>
        ) : (
          <span className="tabular-nums">{dataCounts(t.catalog_tables, t.catalog_columns) ?? "—"}</span>
        ),
    },
    {
      key: "bi",
      label: "BI objects",
      sortable: true,
      className: "tabular-nums",
      value: (t) => t.assignment_count,
      cell: (t) => t.assignment_count,
    },
    {
      key: "clearance",
      label: TERM.clearance,
      sortable: true,
      value: (t) => t.required_roles.length + t.required_groups.length,
      cell: (t) => <ClearanceCell tag={t} />,
    },
    ...(canManage
      ? [
          {
            key: "actions",
            label: "",
            className: "text-right",
            cell: (t: Tag) => (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label={`Actions for ${t.name}`}
                    onPointerDown={(e) => (menuOpener.current = e.currentTarget)}
                    onKeyDown={(e) => (menuOpener.current = e.currentTarget)}
                  >
                    <MoreHorizontal className="h-4 w-4" aria-hidden />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem
                    className="text-destructive focus:text-destructive"
                    onSelect={() => setDeleting(t)}
                  >
                    <Trash2 className="mr-2 h-4 w-4" aria-hidden />
                    Delete
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            ),
          } satisfies Column<Tag>,
        ]
      : []),
  ];

  let body: ReactNode;
  if (tagsQ.isError) {
    body = <TabError what="list of classifications" onRetry={() => void tagsQ.refetch()} />;
  } else if (!rows || !shown) {
    body = <ListSkeleton rows={6} cols={6} />;
  } else if (rows.length === 0) {
    body = <EmptyState icon={Tags} variant="dashed" title={NO_CLASSIFICATION_YET} action={newButton} />;
  } else {
    body = (
      <FilterableTable
        rows={shown}
        columns={columns}
        getId={(t) => t.id}
        initialSort={{ key: "name", dir: "asc" }}
        pageSize={25}
        bordered
        hideSearch
        emptyText={
          <EmptyState
            icon={SearchX}
            title="No classification matches this search."
            action={
              <Button size="sm" variant="outline" onClick={() => setQ("")}>
                Clear search
              </Button>
            }
          />
        }
      />
    );
  }

  return (
    <ListPageShell
      title={TERM.classifications}
      description={`${TERM.classifications} label data and BI objects, and set the ${TERM.clearance.toLowerCase()} each one requires.`}
      action={newButton}
    >
      <div className="space-y-4">
        {rows ? (
          <SummaryStrip
            items={[
              { label: "classifications", singular: "classification", value: rows.length },
              {
                label: "with clearance",
                value: rows.filter((t) => t.required_roles.length + t.required_groups.length > 0).length,
              },
              { label: "unused", value: rows.filter(isUnused).length },
            ]}
          />
        ) : null}
        {rows && rows.length > 0 ? (
          <div className="max-w-xs">
            <Label htmlFor={searchId} className="sr-only">
              Search classifications
            </Label>
            <Input
              id={searchId}
              type="search"
              placeholder="Search classifications…"
              value={q}
              onChange={(e) => setQ(e.target.value)}
            />
          </div>
        ) : null}
        {body}
      </div>
      <NewClassificationDialog open={creating} onOpenChange={setCreating} opener={createOpener} />
      {deleting ? (
        <DeleteTagDialog
          tag={deleting}
          open
          onOpenChange={(o) => {
            if (!o) setDeleting(null);
          }}
          where="the classification's page"
          opener={menuOpener}
        />
      ) : null}
    </ListPageShell>
  );
}

/**
 * New classification (spec §3, FE-PAGE-3): the key follows the name until
 * edited. A refusal keeps the dialog and every input (FE-ACTION-3); a 409 (the
 * key is taken) is shown beside Key (FE-FORM-4). Success opens the new page,
 * without a toast (FE-ACTION-2).
 */
function NewClassificationDialog({
  open,
  onOpenChange,
  opener,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The button that opened it; focus returns there on close (live check D4), the dialog being opened from state. */
  opener: RefObject<HTMLElement | null>;
}) {
  const create = useCreateTag();
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const [key, setKey] = useState("");
  const [keyEdited, setKeyEdited] = useState(false);
  const [description, setDescription] = useState("");
  const [color, setColor] = useState(DEFAULT_COLOR);
  const [keyError, setKeyError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const ids = { name: useId(), key: useId(), keyHelp: useId(), keyError: useId(), description: useId(), color: useId() };

  const close = (next: boolean) => {
    if (!next) {
      setName("");
      setKey("");
      setKeyEdited(false);
      setDescription("");
      setColor(DEFAULT_COLOR);
      setKeyError(null);
      setError(null);
      create.reset();
    }
    onOpenChange(next);
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setKeyError(null);
    setError(null);
    create.mutate(
      { key: key.trim(), name: name.trim(), description: description.trim(), color },
      {
        onSuccess: (t) => {
          close(false);
          navigate(tagUrl(t.id));
        },
        onError: (err) => {
          const message = err.message || "The classification could not be created. Try again.";
          if (err instanceof ApiError && err.status === 409) setKeyError(message);
          else setError(message);
        },
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent
        onCloseAutoFocus={(event) => {
          const element = opener.current?.isConnected ? opener.current : null;
          if (element) {
            event.preventDefault();
            element.focus();
          }
        }}
      >
        <form onSubmit={submit} className="space-y-4">
          <DialogHeader>
            <DialogTitle>New classification</DialogTitle>
            <DialogDescription>A tag to classify tables, columns and BI objects with.</DialogDescription>
          </DialogHeader>
          {error ? (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
          <div className="flex items-end gap-3">
            <div className="flex-1 space-y-1.5">
              <Label htmlFor={ids.name}>Name</Label>
              <Input
                id={ids.name}
                value={name}
                required
                autoFocus
                onChange={(e) => {
                  setName(e.target.value);
                  if (!keyEdited) setKey(slugifyTagKey(e.target.value));
                }}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor={ids.color}>Colour</Label>
              <Input
                id={ids.color}
                type="color"
                className="h-9 w-12 p-1"
                value={color}
                onChange={(e) => setColor(e.target.value)}
              />
            </div>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={ids.key}>Key</Label>
            <Input
              id={ids.key}
              value={key}
              required
              className="font-mono text-xs"
              aria-invalid={keyError ? true : undefined}
              aria-describedby={keyError ? `${ids.keyHelp} ${ids.keyError}` : ids.keyHelp}
              onChange={(e) => {
                setKey(e.target.value);
                setKeyEdited(true);
                setKeyError(null);
              }}
            />
            {keyError ? (
              <p id={ids.keyError} className="text-sm text-destructive">
                {keyError}
              </p>
            ) : null}
            <p id={ids.keyHelp} className="text-xs text-muted-foreground">
              Fixed once created. Follows the name until you edit it. Lowercase letters, digits, <code>-</code>,{" "}
              <code>_</code> and <code>:</code>.
            </p>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={ids.description}>Description</Label>
            <Textarea
              id={ids.description}
              rows={3}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending || !key.trim() || !name.trim()}>
              Create classification
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
