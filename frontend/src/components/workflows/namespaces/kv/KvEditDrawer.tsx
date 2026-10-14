import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { Loader2, RotateCw } from "lucide-react";
import type { KvEntryDto } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { workflowsKeys } from "@/routes/workflows/queryKeys";
import { kvErrorMessage, putKvValue, readKvValue, type KvValueDetail } from "./kvApi";
import { formatInstant } from "./KvTable";
import { KvValueControl } from "./KvValueControl";
import {
  checkDescription,
  checkKey,
  checkTtl,
  encodeValue,
  KV_TYPES,
  kvExpression,
  remainingTtl,
  valueToText,
  type KvType,
} from "./kvValue";

/** The expiration choices (FE-FORM-3): Kestra's "never" or a duration, with the common ones named. */
const TTL_PRESETS = [
  { value: "PT5M", label: "5 minutes" },
  { value: "PT1H", label: "1 hour" },
  { value: "P1D", label: "1 day" },
  { value: "P7D", label: "7 days" },
  { value: "P30D", label: "30 days" },
] as const;

type TtlChoice = "keep" | "never" | "custom" | (typeof TTL_PRESETS)[number]["value"];

interface FormState {
  key: string;
  type: KvType;
  /** The text per type, so switching the type and back keeps what was typed. */
  values: Record<KvType, string>;
  description: string;
  ttl: TtlChoice;
  ttlCustom: string;
}

const emptyValues = (): Record<KvType, string> =>
  Object.fromEntries(KV_TYPES.map((t) => [t, ""])) as Record<KvType, string>;

/**
 * Create a key, or edit one (W7d spec §4.4): key (locked on edit), type,
 * value, description and expiration. An edit first reads the value
 * (`readKvValue`, every digit kept) and fills the form with it.
 */
export function KvEditDrawer({
  ws,
  namespace,
  entry,
  existingKeys,
  onSaved,
  onClose,
}: {
  ws: string;
  namespace: string;
  entry?: KvEntryDto;
  existingKeys: Set<string>;
  onSaved: () => Promise<unknown>;
  onClose: () => void;
}) {
  const detail = useQuery({
    queryKey: workflowsKeys.kvValue(ws, namespace, entry?.key ?? ""),
    queryFn: () => readKvValue(ws, namespace, entry!.key),
    enabled: !!entry,
    // The form is filled from what the server holds now, not from a cached read.
    staleTime: 0,
    gcTime: 0,
  });

  const [askDiscard, setAskDiscard] = useState(false);
  const [dirty, setDirty] = useState(false);
  const requestClose = () => (dirty ? setAskDiscard(true) : onClose());

  let body;
  if (!entry) {
    body = <KvForm ws={ws} namespace={namespace} existingKeys={existingKeys} onSaved={onSaved} onClose={onClose} onDirty={setDirty} />;
  } else if (detail.isError) {
    body = (
      <Alert variant="destructive">
        <AlertDescription className="flex flex-wrap items-center gap-3">
          <span>{kvErrorMessage(detail.error, "The value could not be read")}</span>
          <Button variant="outline" size="sm" onClick={() => void detail.refetch()}>
            <RotateCw aria-hidden />
            Retry
          </Button>
        </AlertDescription>
      </Alert>
    );
  } else if (!detail.data) {
    body = (
      <div className="grid gap-3" aria-hidden>
        <Skeleton className="h-10 w-full" />
        <Skeleton className="h-10 w-48" />
        <Skeleton className="h-24 w-full" />
      </div>
    );
  } else {
    body = (
      <KvForm
        ws={ws}
        namespace={namespace}
        entry={entry}
        stored={detail.data}
        existingKeys={existingKeys}
        onSaved={onSaved}
        onClose={onClose}
        onDirty={setDirty}
      />
    );
  }

  return (
    <>
      <Sheet open onOpenChange={(open) => (open ? undefined : requestClose())}>
        <SheetContent className="max-w-xl">
          <SheetHeader>
            <SheetTitle>{entry ? `Edit ${entry.key}` : "Add a key"}</SheetTitle>
            <SheetDescription>
              {entry
                ? `Saving writes a new revision of the key; flows read it with ${kvExpression(entry.key)}.`
                : `A key of ${namespace}, which its flows and its child namespaces' flows read with {{ kv('key') }}.`}
            </SheetDescription>
          </SheetHeader>
          {body}
        </SheetContent>
      </Sheet>
      <AlertDialog open={askDiscard} onOpenChange={setAskDiscard}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Discard your changes?</AlertDialogTitle>
            <AlertDialogDescription>What you entered is not saved, and closing loses it.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep editing</AlertDialogCancel>
            <Button variant="destructive" onClick={onClose}>
              Discard
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

function initialState(entry: KvEntryDto | undefined, stored: KvValueDetail | undefined): FormState {
  const values = emptyValues();
  if (!entry || !stored) return { key: "", type: "STRING", values, description: "", ttl: "never", ttlCustom: "" };
  values[stored.type] = valueToText(stored.type, stored.value);
  const expiring = stored.expiresAt !== null && remainingTtl(stored.expiresAt) !== null;
  return {
    key: entry.key,
    type: stored.type,
    values,
    description: stored.description ?? "",
    ttl: expiring ? "keep" : "never",
    ttlCustom: "",
  };
}

function KvForm({
  ws,
  namespace,
  entry,
  stored,
  existingKeys,
  onSaved,
  onClose,
  onDirty,
}: {
  ws: string;
  namespace: string;
  entry?: KvEntryDto;
  stored?: KvValueDetail;
  existingKeys: Set<string>;
  onSaved: () => Promise<unknown>;
  onClose: () => void;
  onDirty: (dirty: boolean) => void;
}) {
  const [initial] = useState(() => initialState(entry, stored));
  const [form, setForm] = useState<FormState>(initial);
  const [submitted, setSubmitted] = useState(false);
  const [saving, setSaving] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);
  const [askOverwrite, setAskOverwrite] = useState(false);

  const dirty = JSON.stringify(form) !== JSON.stringify(initial);
  useUnsavedChangesWarning(dirty && !saving);
  const update = (patch: Partial<FormState>) => {
    const next = { ...form, ...patch };
    setForm(next);
    onDirty(JSON.stringify(next) !== JSON.stringify(initial));
  };

  const editing = !!entry;
  const keyError = editing ? null : checkKey(form.key);
  const encoded = encodeValue(form.type, form.values[form.type]);
  const descriptionError = checkDescription(form.description);
  const ttlError = form.ttl === "custom" ? checkTtl(form.ttlCustom) : null;
  const valid = !keyError && encoded.ok && !descriptionError && !ttlError;
  const show = (e: string | null) => (submitted ? e : null);

  const ttlHeader = (): string | null => {
    switch (form.ttl) {
      case "never":
        return null;
      case "keep":
        return stored?.expiresAt ? remainingTtl(stored.expiresAt) : null;
      case "custom":
        return form.ttlCustom.trim();
      default:
        return form.ttl;
    }
  };

  const put = async () => {
    if (!encoded.ok) return;
    setSaving(true);
    setServerError(null);
    try {
      await putKvValue(ws, namespace, form.key, encoded.body, { description: form.description.trim(), ttl: ttlHeader() });
      toast.success(`Saved the key “${form.key}”.`);
      onDirty(false);
      await onSaved();
      onClose();
    } catch (e) {
      // Nothing the user entered is cleared (FE-ACTION-3).
      setServerError(kvErrorMessage(e, "The key was not saved"));
    } finally {
      setSaving(false);
    }
  };

  const save = () => {
    setSubmitted(true);
    if (!valid || saving) return;
    if (!editing && existingKeys.has(form.key)) {
      setAskOverwrite(true);
      return;
    }
    void put();
  };

  const describe = (help: string, error: string | null) => (error ? `${help} ${error}` : help);

  return (
    <>
    <form
      className="flex min-h-0 flex-1 flex-col gap-4"
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        save();
      }}
    >
      <div className="grid min-h-0 flex-1 content-start gap-4 overflow-auto pr-1">
        <div className="grid gap-1.5">
          <Label htmlFor="kv-key">Key</Label>
          <Input
            id="kv-key"
            className="font-mono"
            autoComplete="off"
            readOnly={editing}
            // The drawer was opened by the user to add a key (FE-A11Y-6).
            autoFocus={!editing}
            aria-invalid={show(keyError) ? true : undefined}
            aria-describedby={describe("kv-key-help", show(keyError) && "kv-key-error")}
            value={form.key}
            onChange={(e) => update({ key: e.target.value })}
          />
          <p id="kv-key-help" className="text-xs text-muted-foreground">
            {editing
              ? "A key cannot be renamed: add a new key and delete this one instead."
              : "Letters, digits, dots, dashes and underscores, starting with a letter or a digit."}
          </p>
          {show(keyError) ? (
            <p id="kv-key-error" className="text-sm text-destructive">
              {keyError}
            </p>
          ) : null}
        </div>

        <div className="grid gap-1.5">
          <Label htmlFor="kv-type">Type</Label>
          <Select value={form.type} onValueChange={(t) => update({ type: t as KvType })}>
            <SelectTrigger id="kv-type" className="w-48">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {KV_TYPES.map((t) => (
                <SelectItem key={t} value={t}>
                  {t}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <KvValueControl
          key={form.type}
          type={form.type}
          value={form.values[form.type]}
          onChange={(v) => update({ values: { ...form.values, [form.type]: v } })}
          error={show(encoded.ok ? null : encoded.error)}
        />

        <div className="grid gap-1.5">
          <Label htmlFor="kv-description">Description</Label>
          <Input
            id="kv-description"
            autoComplete="off"
            aria-invalid={show(descriptionError) ? true : undefined}
            aria-describedby={show(descriptionError) ? "kv-description-error" : undefined}
            value={form.description}
            onChange={(e) => update({ description: e.target.value })}
          />
          {show(descriptionError) ? (
            <p id="kv-description-error" className="text-sm text-destructive">
              {descriptionError}
            </p>
          ) : null}
        </div>

        <div className="grid gap-1.5">
          <Label htmlFor="kv-ttl">Expiration</Label>
          <Select value={form.ttl} onValueChange={(v) => update({ ttl: v as TtlChoice })}>
            <SelectTrigger id="kv-ttl" className="w-72" aria-describedby="kv-ttl-help">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {initial.ttl === "keep" && stored?.expiresAt ? (
                <SelectItem value="keep">Keep the current expiration ({formatInstant(stored.expiresAt)})</SelectItem>
              ) : null}
              <SelectItem value="never">Never</SelectItem>
              {TTL_PRESETS.map((p) => (
                <SelectItem key={p.value} value={p.value}>
                  {p.label}
                </SelectItem>
              ))}
              <SelectItem value="custom">Custom TTL</SelectItem>
            </SelectContent>
          </Select>
          <p id="kv-ttl-help" className="text-xs text-muted-foreground">
            The key expires this long after it is saved.
          </p>
          {form.ttl === "custom" ? (
            <div className="grid gap-1.5">
              <Label htmlFor="kv-ttl-custom">TTL</Label>
              <Input
                id="kv-ttl-custom"
                className="w-48 font-mono"
                autoComplete="off"
                placeholder="PT12H"
                aria-invalid={show(ttlError) ? true : undefined}
                aria-describedby={describe("kv-ttl-custom-help", show(ttlError) && "kv-ttl-error")}
                value={form.ttlCustom}
                onChange={(e) => update({ ttlCustom: e.target.value })}
              />
              <p id="kv-ttl-custom-help" className="text-xs text-muted-foreground">
                An ISO-8601 duration: PT30M, PT12H, P7D, P1DT12H.
              </p>
              {show(ttlError) ? (
                <p id="kv-ttl-error" className="text-sm text-destructive">
                  {ttlError}
                </p>
              ) : null}
            </div>
          ) : null}
        </div>
      </div>

      {serverError ? (
        <Alert variant="destructive">
          <AlertDescription>{serverError}</AlertDescription>
        </Alert>
      ) : null}
      <div className="flex shrink-0 justify-end gap-2">
        <Button type="button" variant="outline" onClick={onClose} disabled={saving}>
          Cancel
        </Button>
        <Button type="submit" disabled={saving}>
          {saving ? <Loader2 aria-hidden className="animate-spin" /> : null}
          Save
        </Button>
      </div>
    </form>

      {/* Outside the form: a portal's events still bubble through the React tree. */}
      <AlertDialog open={askOverwrite} onOpenChange={setAskOverwrite}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Overwrite “{form.key}”?</AlertDialogTitle>
            <AlertDialogDescription>
              A key named {form.key} already exists in {namespace}. Saving replaces its value, type, description and
              expiration; its earlier revisions are kept.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <Button
              variant="destructive"
              onClick={() => {
                setAskOverwrite(false);
                void put();
              }}
            >
              Overwrite
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
