import { useId, useMemo, useRef, useState, type FormEvent, type ReactNode, type RefObject } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { MoreHorizontal, Plus, SearchX, Tags, Trash2 } from "lucide-react";
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
import { useCanEditCatalogTags } from "@/routes/governance/ClassificationChip";
import { RequiredMark, TabError } from "@/routes/governance/tabParts";
import { tagKeyInvalid, tagKeyTaken, tagNameTaken } from "@/routes/governance/apiErrors";
import { DeleteTagDialog } from "@/routes/governance/classifications/DeleteTagDialog";
import { tagUrl } from "@/routes/governance/classifications/tagUrl";
import {
  ClearanceCell,
  ColourField,
  dataCounts,
  DEFAULT_COLOR,
  NameTaken,
  nameHolder,
  slugifyTagKey,
  TAG_KEY_PATTERN,
  TAG_KEY_RULE_ERROR,
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
  // The header's New classification: where focus goes once a deleted row (and its Actions button) is gone.
  const headerNew = useRef<HTMLButtonElement | null>(null);
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

  const newButtonAt = (ref?: RefObject<HTMLButtonElement | null>) =>
    canManage ? (
      <Button
        ref={ref}
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
  const newButton = newButtonAt();

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
    body = (
      <EmptyState
        icon={Tags}
        variant="dashed"
        title={NO_CLASSIFICATION_YET}
        action={<HowClassificationWorks newButton={newButton} />}
      />
    );
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
      action={newButtonAt(headerNew)}
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
          // After the closed dialog's own focus return (Radix defers it a task), which a fast answer can precede.
          onDeleted={() => window.setTimeout(() => headerNew.current?.focus(), 0)}
        />
      ) : null}
    </ListPageShell>
  );
}

export const CREATING_NEEDS_TAG_MANAGE = "Creating needs the tag:manage permission.";
/** Step 2 happens on the Data page, whose Add classification needs `catalog_registry:update` (FE-ACTION-4). */
export const ATTACHING_NEEDS_CATALOG_UPDATE = "Attaching needs the catalog_registry:update permission.";

/**
 * The first-run steps (spec C3): create, attach in Data, then add a rule or
 * set clearance on the classification's page. Step 1 carries New
 * classification for a `tag:manage` holder; anyone else is told what
 * creating needs, and a viewer who cannot attach in Data what attaching
 * needs (FE-ACTION-4).
 */
function HowClassificationWorks({ newButton }: { newButton: ReactNode }) {
  const canAttach = useCanEditCatalogTags();
  return (
    <div className="space-y-3 text-left">
      <ol aria-label="How classification works" className="list-decimal space-y-1 pl-5 text-sm text-muted-foreground">
        <li>Create a classification.</li>
        <li>
          {"Attach it to tables or columns in "}
          <Link to="/governance/data" className="font-medium text-primary hover:underline">
            Data
          </Link>
          .
        </li>
        <li>Add a rule (mask or row filter) or set clearance on the classification&apos;s page.</li>
      </ol>
      {canAttach ? null : <p className="text-xs text-muted-foreground">{ATTACHING_NEEDS_CATALOG_UPDATE}</p>}
      <div className="flex justify-center">
        {newButton ?? <p className="text-xs text-muted-foreground">{CREATING_NEEDS_TAG_MANAGE}</p>}
      </div>
    </div>
  );
}

/** The create dialog's field errors: each sits under its own field (FE-FORM-4). */
type FieldError = ReactNode | null;

/**
 * New classification (spec §3, FE-PAGE-3, C10): the key follows the name
 * until edited. The key is checked as you type against the server's rule
 * (`TAG_KEY_PATTERN`), and the name against the loaded list
 * (case-insensitive), so Create waits, `aria-disabled` with the reason tied,
 * while either is wrong or empty. The server stays the authority: its coded
 * refusals land under their own field (`tag_key_taken`, `tag_key_invalid`
 * under Key; `tag_name_taken` under Name), anything else over the form. A
 * refusal keeps the dialog and every input (FE-ACTION-3). Success opens the
 * new page, without a toast (FE-ACTION-2).
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
  const tags = useTags();
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const [key, setKey] = useState("");
  const [keyEdited, setKeyEdited] = useState(false);
  const [description, setDescription] = useState("");
  const [color, setColor] = useState(DEFAULT_COLOR);
  const [serverKeyError, setServerKeyError] = useState<FieldError>(null);
  const [serverNameError, setServerNameError] = useState<FieldError>(null);
  const [error, setError] = useState<string | null>(null);
  const ids = {
    name: useId(),
    nameError: useId(),
    key: useId(),
    keyHelp: useId(),
    keyError: useId(),
    description: useId(),
    color: useId(),
    waits: useId(),
  };

  const close = (next: boolean) => {
    if (!next) {
      setName("");
      setKey("");
      setKeyEdited(false);
      setDescription("");
      setColor(DEFAULT_COLOR);
      setServerKeyError(null);
      setServerNameError(null);
      setError(null);
      create.reset();
    }
    onOpenChange(next);
  };

  const trimmedKey = key.trim();
  const holder = nameHolder(tags.data, name);
  const keyError: FieldError =
    serverKeyError ?? (trimmedKey && !TAG_KEY_PATTERN.test(trimmedKey) ? TAG_KEY_RULE_ERROR : null);
  const nameError: FieldError = serverNameError ?? (holder ? <NameTaken holder={holder} /> : null);
  const empty = !name.trim() || !trimmedKey;
  const waiting = empty || !!keyError || !!nameError || create.isPending;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (waiting) return;
    setServerKeyError(null);
    setServerNameError(null);
    setError(null);
    create.mutate(
      { key: trimmedKey, name: name.trim(), description: description.trim(), color },
      {
        onSuccess: (t) => {
          close(false);
          navigate(tagUrl(t.id));
        },
        onError: (err) => {
          const keyTaken = tagKeyTaken(err);
          const nameTaken = tagNameTaken(err);
          const keyInvalid = tagKeyInvalid(err);
          if (keyTaken) {
            setServerKeyError(
              <>
                {`Key ${keyTaken.key} is taken by `}
                <Link to={tagUrl(keyTaken.existing.id)} className="font-medium underline">
                  {keyTaken.existing.name}
                </Link>
                .
              </>,
            );
          } else if (nameTaken) {
            setServerNameError(<NameTaken holder={nameTaken.existing} />);
          } else if (keyInvalid) {
            setServerKeyError(keyInvalid.error);
          } else {
            setError(err.message || "The classification could not be created. Try again.");
          }
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
            <DialogDescription>A label to classify tables, columns and BI objects with.</DialogDescription>
          </DialogHeader>
          {error ? (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
          <div className="space-y-1.5">
            <Label htmlFor={ids.name}>
              Name
              <RequiredMark />
            </Label>
            <Input
              id={ids.name}
              value={name}
              aria-required="true"
              autoFocus
              aria-invalid={nameError ? true : undefined}
              aria-describedby={nameError ? ids.nameError : undefined}
              onChange={(e) => {
                setName(e.target.value);
                setServerNameError(null);
                if (!keyEdited) {
                  setKey(slugifyTagKey(e.target.value));
                  setServerKeyError(null);
                }
              }}
            />
            {nameError ? (
              <p id={ids.nameError} className="text-sm text-destructive">
                {nameError}
              </p>
            ) : null}
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={ids.key}>
              {TERM.key}
              <RequiredMark />
            </Label>
            <Input
              id={ids.key}
              value={key}
              aria-required="true"
              className="font-mono text-xs"
              aria-invalid={keyError ? true : undefined}
              aria-describedby={keyError ? `${ids.keyError} ${ids.keyHelp}` : ids.keyHelp}
              onChange={(e) => {
                setKey(e.target.value);
                setKeyEdited(true);
                setServerKeyError(null);
              }}
            />
            {keyError ? (
              <p id={ids.keyError} className="text-sm text-destructive">
                {keyError}
              </p>
            ) : null}
            <p id={ids.keyHelp} className="text-xs text-muted-foreground">
              Fixed once created. Follows the name until you edit it. Lowercase letters, digits, - and :.
            </p>
          </div>
          <ColourField id={ids.color} color={color} onChange={setColor} />
          <div className="space-y-1.5">
            <Label htmlFor={ids.description}>Description</Label>
            <Textarea
              id={ids.description}
              rows={3}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
          <DialogFooter className="items-center">
            {empty ? (
              <p id={ids.waits} className="text-xs text-muted-foreground">
                Enter a name and a key.
              </p>
            ) : null}
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button
              type="submit"
              aria-disabled={waiting ? true : undefined}
              aria-describedby={
                empty ? ids.waits : nameError ? ids.nameError : keyError ? ids.keyError : undefined
              }
            >
              Create classification
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
