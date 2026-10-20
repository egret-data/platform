/**
 * An engine's Ranger plugin credential (spec 2026-10-20-ranger-surface-auth
 * A10/A11): the one-time dialog that hands a new credential over, and the
 * "Ranger credential" section that rotates it and, for an external engine,
 * retires the previous one.
 *
 * THE PASSWORD LIVES IN COMPONENT STATE ONLY. It arrives in a mutation's
 * response (registration, or an external engine's rotation) and is copied into
 * the state of the component that shows it; the mutation is created with
 * `gcTime: 0` and `reset()` right after, so React Query's mutation cache does
 * not keep the response once nothing observes it. It is never put in the URL,
 * storage, a query cache entry, a toast or a log line, and the dialog that
 * shows it drops it when it closes. A reload cannot bring it back, by design.
 */
import { useEffect, useId, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Check, Copy, Eye, EyeOff, KeyRound, Loader2 } from "lucide-react";
import { toast } from "sonner";

import { ApiError } from "@/api/client";
import type { Engine } from "@/api/controlPlane";
import {
  getControlPlaneEnginesRangerPluginConfigQueryKey,
  useControlPlaneEnginesRetirePreviousRangerCredential,
  useControlPlaneEnginesRotateRangerCredential,
} from "@/api/generated/control-plane-engines/control-plane-engines";
import type { RangerCredentialDto } from "@/api/generated/model";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { enginesQueryKey } from "./ControlPlaneEngines";
import { ServerRefusal, isExternal, useCan } from "./externalEngine";
import { engineDetailKey } from "./queryKeys";

/** The Ranger plugin name in the property keys: the engine type. */
function pluginOf(engineType: string): string {
  return engineType === "starrocks" ? "starrocks" : "trino";
}

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function CopyButton({ value, what }: { value: string; what: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  // A pending reset must not fire into an unmounted button (the dialog closes on Done).
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(false), 2000);
    } catch {
      toast.error(`Could not copy the ${what} to the clipboard. Select the text and copy it.`);
    }
  };
  return (
    <>
      <Button type="button" size="sm" variant="outline" onClick={() => void copy()} aria-label={`Copy ${what}`}>
        {copied ? <Check className="h-3.5 w-3.5" aria-hidden /> : <Copy className="h-3.5 w-3.5" aria-hidden />}
        {copied ? "Copied" : "Copy"}
      </Button>
      {/* The button's name stays "Copy …", so the result is announced here. */}
      <span role="status" aria-live="polite" className="sr-only">
        {copied ? `Copied the ${what}` : ""}
      </span>
    </>
  );
}

/** One read-only, selectable value with its label and a copy control. */
function CredentialField({
  id,
  label,
  value,
  what,
  secret = false,
  hint,
}: {
  id: string;
  label: string;
  value: string;
  what: string;
  secret?: boolean;
  hint?: string;
}) {
  const [revealed, setRevealed] = useState(false);
  return (
    <div className="space-y-1.5">
      <Label htmlFor={id}>{label}</Label>
      <div className="flex items-center gap-2">
        <Input
          id={id}
          readOnly
          value={value}
          type={secret && !revealed ? "password" : "text"}
          autoComplete="off"
          spellCheck={false}
          className="font-mono text-xs"
          onFocus={(e) => e.currentTarget.select()}
          aria-describedby={hint ? `${id}-hint` : undefined}
        />
        {secret && (
          <Button
            type="button"
            size="sm"
            variant="outline"
            aria-pressed={revealed}
            aria-label={revealed ? `Hide ${what}` : `Show ${what}`}
            onClick={() => setRevealed((r) => !r)}
          >
            {revealed ? <EyeOff className="h-3.5 w-3.5" aria-hidden /> : <Eye className="h-3.5 w-3.5" aria-hidden />}
            {revealed ? "Hide" : "Show"}
          </Button>
        )}
        <CopyButton value={value} what={what} />
      </div>
      {hint && (
        <p id={`${id}-hint`} className="text-xs text-muted-foreground">
          {hint}
        </p>
      )}
    </div>
  );
}

/**
 * The credential, shown once. It cannot be dismissed by Escape or a click
 * outside: the only way out is the footer button, enabled once the reader has
 * said they saved it, because closing destroys the only copy egret will show.
 */
export function OneTimeCredentialDialog({
  credential,
  engineType,
  title,
  intro,
  after,
  doneLabel,
  onDone,
}: {
  credential: RangerCredentialDto | null;
  engineType: string;
  title: string;
  intro: string;
  /** What to do once it is installed (a sentence after the install steps). */
  after?: string;
  doneLabel: string;
  onDone: () => void;
}) {
  const base = useId();
  const [saved, setSaved] = useState(false);
  const plugin = pluginOf(engineType);
  const finish = () => {
    setSaved(false);
    onDone();
  };
  return (
    // `onOpenChange` is deliberately inert: Escape and a click outside would
    // otherwise close it and lose the password. `finish` is the only exit.
    <AlertDialog open={credential !== null} onOpenChange={() => {}}>
      <AlertDialogContent className="sm:max-w-2xl" onEscapeKeyDown={(e) => e.preventDefault()}>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{intro}</AlertDialogDescription>
        </AlertDialogHeader>
        {credential && (
          <div className="space-y-4">
            <CredentialField id={`${base}-user`} label="Username" value={credential.username} what="username" />
            <CredentialField
              id={`${base}-password`}
              label="Password"
              value={credential.password}
              what="password"
              secret
            />
            <CredentialField
              id={`${base}-credential-id`}
              label="Credential id"
              value={credential.credentialId}
              what="credential id"
              hint="Names this credential when you confirm it is installed. It is not a secret and grants nothing."
            />
            <div className="space-y-1.5 rounded-md border p-3 text-sm">
              <p className="font-medium">Install it on the engine</p>
              <ol className="list-decimal space-y-1 pl-5">
                <li>
                  In <span className="font-mono">ranger-{plugin}-security.xml</span>, set{" "}
                  <span className="break-all font-mono">ranger.plugin.{plugin}.policy.rest.client.username</span> to
                  the username and{" "}
                  <span className="break-all font-mono">ranger.plugin.{plugin}.policy.rest.client.password</span> to
                  the password.
                </li>
                <li>Restart the engine so its Ranger plugin reads them.</li>
              </ol>
              <p className="text-muted-foreground">
                The rest of the plugin files, with a placeholder where the password goes, are on the engine&apos;s
                Plugin config tab.
                {after ? ` ${after}` : ""}
              </p>
            </div>
          </div>
        )}
        <div className="flex items-start gap-2">
          <Checkbox id={`${base}-saved`} checked={saved} onCheckedChange={(v) => setSaved(v === true)} />
          <Label htmlFor={`${base}-saved`} className="font-normal leading-snug">
            I have saved the password and the credential id
          </Label>
        </div>
        <AlertDialogFooter className="items-center gap-2">
          <p id={`${base}-why`} className="text-xs text-muted-foreground sm:mr-auto">
            {saved ? "" : "egret does not show this password again. Confirm you saved it to continue."}
          </p>
          <Button type="button" disabled={!saved} aria-describedby={`${base}-why`} onClick={finish}>
            {doneLabel}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** The `installed` token: the credential id the last rotation returned. Never the password. */
export interface RotationToken {
  value: string | null;
  set: (id: string | null) => void;
}

/**
 * Hold the rotation token for one engine, in React state only (not the URL or
 * storage: a reload loses it, and the retire dialog then asks for the id).
 * Keyed by engine id, so moving to another engine's page drops it.
 */
export function useRotationToken(engineId: string): RotationToken {
  const [held, setHeld] = useState<{ engineId: string; id: string } | null>(null);
  return {
    value: held && held.engineId === engineId ? held.id : null,
    set: (id) => setHeld(id === null ? null : { engineId, id }),
  };
}

/**
 * The "Ranger credential" section of an engine's Overview: its state, Rotate,
 * and — for an external engine with a rotation pending — Retire previous.
 *
 * Provisioned: egret applies the new credential (the pods roll) and retires the
 * previous one itself once the rollout is observed complete; this page polls
 * the engine, so the pending state clears by itself.
 *
 * External: the new credential is shown once, the operator installs it, then
 * confirms here. The confirmation names WHICH credential is installed
 * (`installed`): the id the rotation returned, held in this component's state;
 * after a reload the operator enters the id they saved with the password.
 */
export function RangerCredentialSection({
  engine,
  rotation,
}: {
  engine: Pick<Engine, "id" | "type" | "origin" | "rangerCredentialPending" | "rangerCredentialId">;
  /**
   * Where the `installed` token lives. A page whose tabs unmount this section
   * (the operator installs the credential on the Plugin config tab) passes a
   * holder that outlives the tab panel: {@link useRotationToken}. Without one,
   * the token lives in this section's own state.
   */
  rotation?: RotationToken;
}) {
  const queryClient = useQueryClient();
  const external = isExternal(engine);
  const canUpdate = useCan("engine:update");
  const pending = engine.rangerCredentialPending === true;
  const currentId = engine.rangerCredentialId ?? null;
  const base = useId();

  const [confirmRotate, setConfirmRotate] = useState(false);
  const [shown, setShown] = useState<RangerCredentialDto | null>(null);
  // The id the last rotation in THIS mount returned: the `installed` token.
  const ownToken = useRotationToken(engine.id);
  const { value: rotatedId, set: setRotatedId } = rotation ?? ownToken;
  const [retireOpen, setRetireOpen] = useState(false);
  const [typedId, setTypedId] = useState("");
  const [failure, setFailure] = useState<unknown>(null);

  const settle = () => {
    void queryClient.invalidateQueries({ queryKey: engineDetailKey(engine.id) });
    void queryClient.invalidateQueries({ queryKey: enginesQueryKey });
    void queryClient.invalidateQueries({ queryKey: getControlPlaneEnginesRangerPluginConfigQueryKey(engine.id) });
  };

  const rotate = useControlPlaneEnginesRotateRangerCredential<ApiError>({
    mutation: {
      // Not kept once unobserved: the 200 body carries a password.
      gcTime: 0,
      onSuccess: (res) => {
        setFailure(null);
        setConfirmRotate(false);
        settle();
        if (res.status === 200) {
          // The dialog is the confirmation; no toast.
          const { username, password, credentialId } = res.data;
          setShown({ username, password, credentialId });
          setRotatedId(credentialId);
        } else {
          toast.success(
            "Credential rotated. The engine's pods are restarting; the previous credential stays valid until the rollout completes.",
          );
        }
        rotate.reset();
      },
      onError: (e) => {
        setConfirmRotate(false);
        setFailure(e);
      },
    },
  });

  const retire = useControlPlaneEnginesRetirePreviousRangerCredential<ApiError>({
    mutation: {
      onSuccess: () => {
        setFailure(null);
        setRetireOpen(false);
        setRotatedId(null);
        setTypedId("");
        settle();
        toast.success("Previous credential retired. Only the current one authenticates now.");
      },
      onError: (e) => {
        setRetireOpen(false);
        setFailure(e);
      },
    },
  });

  const installed = rotatedId ?? typedId.trim();
  const installedOk = UUID_RE.test(installed);

  const rotateBlocked = pending
    ? external
      ? "Rotation is blocked until you retire the previous credential."
      : "Rotation is blocked until the rollout of the current credential completes and egret retires the previous one."
    : !external && !currentId
      ? "This engine has no Ranger credential yet: egret creates one when the engine is applied bound to a policy profile."
      : null;

  let state: { badge: string; variant: "secondary" | "outline"; text: string };
  if (pending)
    state = {
      badge: "Previous credential pending",
      variant: "outline",
      text: external
        ? "Both the current and the previous credential authenticate until you confirm the engine runs the current one."
        : "Both credentials authenticate until the rollout of the current one completes; then egret retires the previous one. This updates by itself.",
    };
  else if (currentId)
    state = { badge: "Active", variant: "secondary", text: "One credential authenticates this engine's plugin." };
  else
    state = {
      badge: "No credential",
      variant: "outline",
      text: external
        ? "This engine was registered before engine credentials existed. Rotate to create one, then install it on the engine."
        : "egret creates one when the engine is applied bound to a policy profile.",
    };

  return (
    <section className="space-y-3 rounded-lg border p-4" aria-labelledby={`${base}-h`}>
      <h2 id={`${base}-h`} className="text-base font-semibold">
        Ranger credential
      </h2>
      <p className="text-sm text-muted-foreground">
        The username and password this engine&apos;s Ranger plugin presents to egret when it downloads policies.
      </p>
      <dl className="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[10rem_1fr]">
        <dt className="text-muted-foreground">Status</dt>
        <dd className="space-y-1">
          <Badge variant={state.variant}>{state.badge}</Badge>
          <p className="text-muted-foreground">{state.text}</p>
        </dd>
        {/* Hidden while the retire dialog asks for the installed id: it must come
            from what the operator saved, not be copied from here. */}
        {currentId && !(retireOpen && !rotatedId) && (
          <>
            <dt className="text-muted-foreground">Current credential id</dt>
            <dd className="break-all font-mono text-xs" data-testid="ranger-credential-id">
              {currentId}
            </dd>
          </>
        )}
      </dl>

      {failure !== null && <ServerRefusal error={failure} testId="ranger-credential-error" />}

      {canUpdate ? (
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            variant="outline"
            disabled={!!rotateBlocked || rotate.isPending}
            aria-describedby={`${base}-rotate-why`}
            onClick={() => setConfirmRotate(true)}
          >
            {rotate.isPending ? <Loader2 className="h-4 w-4 animate-spin" /> : <KeyRound className="h-4 w-4" />}
            Rotate credential
          </Button>
          {external && pending && (
            <Button type="button" disabled={retire.isPending} onClick={() => setRetireOpen(true)}>
              {retire.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Retire previous credential
            </Button>
          )}
          <p id={`${base}-rotate-why`} className="text-xs text-muted-foreground">
            {rotateBlocked ?? ""}
          </p>
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">
          Rotating the credential needs the engine:update permission, which this account does not have. A platform
          administrator can grant it.
        </p>
      )}

      <AlertDialog open={confirmRotate} onOpenChange={(o) => !rotate.isPending && setConfirmRotate(o)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Rotate the Ranger credential?</AlertDialogTitle>
            <AlertDialogDescription>
              {external
                ? "egret creates a new credential and shows it once. The engine keeps working with the current one until you install the new one and confirm it here; then the current one stops authenticating. You cannot rotate again until you confirm."
                : `egret creates a new credential and applies it, so the engine's pods restart (${engine.type === "starrocks" ? "the FE" : "the coordinator and workers"}) and running queries are interrupted. The current credential stays valid until the rollout completes; then egret retires it.`}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={rotate.isPending}>Cancel</AlertDialogCancel>
            <Button type="button" disabled={rotate.isPending} onClick={() => rotate.mutate({ id: engine.id })}>
              {rotate.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Rotate credential
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={retireOpen} onOpenChange={(o) => !retire.isPending && setRetireOpen(o)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Retire the previous credential?</AlertDialogTitle>
            <AlertDialogDescription>
              Do this only once the engine runs the new credential. The previous one stops authenticating, so an
              engine still using it can no longer download policies.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {rotatedId ? (
            <p className="text-sm">
              You confirm the engine runs the credential{" "}
              <span className="break-all font-mono text-xs">{rotatedId}</span>, which the rotation on this page
              returned.
            </p>
          ) : (
            <div className="space-y-1.5">
              <Label htmlFor={`${base}-installed`}>Credential id of the installed credential</Label>
              <Input
                id={`${base}-installed`}
                value={typedId}
                onChange={(e) => setTypedId(e.target.value)}
                autoComplete="off"
                spellCheck={false}
                className="font-mono text-xs"
                aria-invalid={typedId.trim() !== "" && !installedOk}
                aria-describedby={`${base}-installed-hint`}
              />
              <p id={`${base}-installed-hint`} className="text-xs text-muted-foreground">
                Enter the id shown with the password at rotation, from your own record of the credential you
                installed. Do not copy the engine&apos;s current credential id from this page unless that is the
                credential the engine runs: egret retires the previous credential only if this is the current one,
                and an engine still using the previous one then can no longer download policies. If you lost the new
                password before installing it, retiring breaks the engine&apos;s policy download until you rotate
                again and install that one.
              </p>
            </div>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={retire.isPending}>Cancel</AlertDialogCancel>
            <Button
              type="button"
              disabled={!installedOk || retire.isPending}
              onClick={() => retire.mutate({ id: engine.id, data: { installed } })}
            >
              {retire.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Retire previous credential
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <OneTimeCredentialDialog
        credential={shown}
        engineType={engine.type}
        title="New Ranger credential"
        intro="Copy it now: egret shows this password once. The previous credential keeps working until you confirm the new one is installed."
        after="Once the engine runs it, choose Retire previous credential in the Ranger credential section."
        doneLabel="Done"
        onDone={() => setShown(null)}
      />
    </section>
  );
}
