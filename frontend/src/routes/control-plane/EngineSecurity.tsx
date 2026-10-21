import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { Plus, Trash2 } from "lucide-react";

import { type Secret, listSecrets } from "@/api/controlPlane";
import { pickableSecrets } from "@/lib/systemSecrets";
import { ApiError } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Switch } from "@/components/ui/switch";
import { ConfigSection } from "@/components/form/ConfigSection";
import { ConfigTable, ConfigTableRow } from "@/components/config/ConfigTable";
import { type Kv, KeyValueEditor } from "./EngineResourceGroups";
import type { DelegationMode, PasswordUserForm } from "./useEngineConfigForm";

/**
 * "Exposure & auth" security config: password users (backed by egret secrets),
 * egret-JWT acceptance (JWKS URI + required audience), and coordinator ingress
 * exposure (host, ingress class, TLS secret, and extra annotations).
 */
export function EngineSecurity(props: {
  passwordUsers: PasswordUserForm[];
  setPasswordUsers: React.Dispatch<React.SetStateAction<PasswordUserForm[]>>;
  jwtEnabled: boolean;
  setJwtEnabled: (v: boolean) => void;
  delegationMode: DelegationMode;
  setDelegationMode: (v: DelegationMode) => void;
  /**
   * Mode PERSISTED on the stored engine (not the live `delegationMode`, which moves
   * as the admin picks). An engine already running jwt/impersonate can't be safely
   * downgraded to open — the Trino update path never tears down `auth.jwt` /
   * rematerializes — so "None" is disabled for it. Empty for a new/open engine.
   */
  storedDelegationMode: DelegationMode;
  jwksUri: string;
  setJwksUri: (v: string) => void;
  requiredAudience: string;
  setRequiredAudience: (v: string) => void;
  ingressEnabled: boolean;
  setIngressEnabled: (v: boolean) => void;
  ingressHost: string;
  setIngressHost: (v: string) => void;
  ingressClassName: string;
  setIngressClassName: (v: string) => void;
  ingressTlsSecret: string;
  setIngressTlsSecret: (v: string) => void;
  ingressTlsCertSecretRef: string;
  setIngressTlsCertSecretRef: (v: string) => void;
  ingressAnnotations: Kv[];
  setIngressAnnotations: React.Dispatch<React.SetStateAction<Kv[]>>;
}) {
  const {
    passwordUsers,
    setPasswordUsers,
    jwtEnabled,
    setJwtEnabled,
    delegationMode,
    setDelegationMode,
    storedDelegationMode,
    jwksUri,
    setJwksUri,
    requiredAudience,
    setRequiredAudience,
    ingressEnabled,
    setIngressEnabled,
    ingressHost,
    setIngressHost,
    ingressClassName,
    setIngressClassName,
    // `ingressTlsSecret`/`setIngressTlsSecret` (raw k8s Secret name) stay in the
    // props channel for back-compat but are no longer rendered — the typed
    // `kubernetes.io/tls` picker below (ingressTlsCertSecretRef) replaces them.
    ingressTlsCertSecretRef,
    setIngressTlsCertSecretRef,
    ingressAnnotations,
    setIngressAnnotations,
  } = props;

  const secrets = useQuery<Secret[], ApiError>({
    queryKey: ["cp", "secrets"],
    queryFn: listSecrets,
    retry: false,
  });
  const pickable = pickableSecrets(secrets.data ?? []);

  // A stored jwt/impersonate engine is "already delegated" — the "None" opt-out
  // is disabled for it (see the `storedDelegationMode` prop doc). Mirrors
  // `StarRocksSecuritySection`'s `hasDelegation`.
  const hasDelegation =
    storedDelegationMode === "jwt" || storedDelegationMode === "impersonate";

  return (
    <div className="space-y-5">
      {/* Password users */}
      <ConfigSection
        title="Password users"
        description="Each user signs in with a username and the password held in the selected egret secret (hashed server-side)."
        docsUrl="https://trino.io/docs/current/security/password-file.html"
      >
        <div className="space-y-2">
          {passwordUsers.map((row, i) => (
            <div key={i} className="flex items-center gap-2">
              <Input
                value={row.username}
                placeholder="username"
                aria-label="Username"
                onChange={(e) =>
                  setPasswordUsers(
                    passwordUsers.map((r, j) =>
                      j === i ? { ...r, username: e.target.value } : r,
                    ),
                  )
                }
              />
              <Select
                value={row.secretRef}
                onValueChange={(v) =>
                  setPasswordUsers(
                    passwordUsers.map((r, j) =>
                      j === i ? { ...r, secretRef: v } : r,
                    ),
                  )
                }
              >
                <SelectTrigger className="w-full" aria-label="Password secret">
                  <SelectValue placeholder="Password secret" />
                </SelectTrigger>
                <SelectContent>
                  {pickable.length === 0 ? (
                    <SelectItem value="__none__" disabled>
                      No secrets
                    </SelectItem>
                  ) : (
                    pickable.map((s) => (
                      <SelectItem key={s.id} value={s.id}>
                        {s.name}
                      </SelectItem>
                    ))
                  )}
                </SelectContent>
              </Select>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="shrink-0 text-muted-foreground"
                onClick={() =>
                  setPasswordUsers(
                    passwordUsers.filter((_, j) => j !== i),
                  )
                }
                aria-label="Remove user"
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
          ))}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() =>
              setPasswordUsers([
                ...passwordUsers,
                { username: "", secretRef: "" },
              ])
            }
          >
            <Plus className="h-4 w-4" />
            Add user
          </Button>
          {pickable.length === 0 && (
            <p className="text-xs text-muted-foreground">
              No secrets yet.{" "}
              <Link
                to="/control-plane/secrets"
                className="text-primary hover:underline"
              >
                Create a secret
              </Link>{" "}
              to hold a user password.
            </p>
          )}
        </div>
      </ConfigSection>

      {/* JWT */}
      <ConfigSection
        title="JWT authentication"
        description="Also accept egret-issued JWTs (e.g. from notebooks) as client credentials."
        docsUrl="https://trino.io/docs/current/security/jwt.html"
      >
        <ConfigTable>
          <ConfigTableRow
            label="http-server.authentication.type"
            mono
            details="Add JWT to the coordinator authentication types so egret-issued JWTs are accepted."
            htmlFor="ced-jwt-enabled"
          >
            <Switch
              id="ced-jwt-enabled"
              checked={jwtEnabled}
              onCheckedChange={setJwtEnabled}
            />
          </ConfigTableRow>
          {jwtEnabled && (
            <>
              <ConfigTableRow
                label="http-server.authentication.jwt.key-file"
                mono
                details="JWKS endpoint whose public keys verify incoming JWTs (blank = egret's default IdP)."
                htmlFor="ced-jwt-jwks"
              >
                <Input
                  id="ced-jwt-jwks"
                  className="w-full"
                  value={jwksUri}
                  placeholder="default IdP JWKS"
                  onChange={(e) => setJwksUri(e.target.value)}
                />
              </ConfigTableRow>
              <ConfigTableRow
                label="http-server.authentication.jwt.required-audience"
                mono
                details="If set, only JWTs whose aud claim matches this value are accepted."
                htmlFor="ced-jwt-aud"
              >
                <Input
                  id="ced-jwt-aud"
                  className="w-full"
                  value={requiredAudience}
                  placeholder="optional"
                  onChange={(e) => setRequiredAudience(e.target.value)}
                />
              </ConfigTableRow>
            </>
          )}
        </ConfigTable>
      </ConfigSection>

      {/* Per-user delegation mode */}
      <ConfigSection
        title="Per-user delegation"
        description="How delegated BI queries run AS the calling user. Applies to databases with per-user delegation enabled."
        docsUrl="https://trino.io/docs/current/security/user-mapping.html"
      >
        <RadioGroup
          value={delegationMode}
          onValueChange={(v) => setDelegationMode(v as DelegationMode)}
          aria-label="Delegation mode"
          className="gap-2"
        >
          {(
            [
              {
                value: "",
                title: "Auto — JWT, else impersonate (recommended)",
                desc: "egret uses per-user JWT when the coordinator can verify it, and otherwise degrades to impersonation automatically. Best default for most engines.",
              },
              {
                value: "jwt",
                title: "JWT (per-user)",
                desc: "Each query uses the caller's own short-lived JWT, bound to their identity over TLS. No shared credential. OIDC users only.",
              },
              {
                value: "impersonate",
                title: "Impersonate (service user)",
                desc: "egret connects as a trusted service account and runs the query as the caller. Works for any signed-in user; runs over TLS.",
              },
              {
                value: "none",
                title: "None (open engine)",
                desc: "No per-user delegation — every query runs with the connection's own configured credential.",
              },
            ] as const
          ).map((opt) => {
            const selected = delegationMode === opt.value;
            const optKey = opt.value || "auto";
            // Downgrading an already-delegated engine (stored jwt/impersonate) to
            // open isn't backend-supported (no auth.jwt teardown / rematerialize on
            // the Trino update path), so disable "None" for it. Switching jwt⇄
            // impersonate stays allowed; a new/open engine keeps every option.
            const disabled = opt.value === "none" && hasDelegation;
            return (
              <Label
                key={optKey}
                htmlFor={`deleg-${optKey}`}
                className={`flex items-start gap-3 rounded-lg border p-3 font-normal transition-colors ${
                  disabled
                    ? "cursor-not-allowed border-border opacity-60"
                    : selected
                      ? "cursor-pointer border-primary bg-primary/5"
                      : "cursor-pointer border-border hover:bg-muted/50"
                }`}
              >
                <RadioGroupItem
                  id={`deleg-${optKey}`}
                  value={opt.value}
                  disabled={disabled}
                  className="mt-0.5"
                />
                <span className="space-y-1">
                  <span className="block text-sm font-medium text-foreground">
                    {opt.title}
                  </span>
                  <span className="block text-xs text-muted-foreground">
                    {opt.desc}
                  </span>
                </span>
              </Label>
            );
          })}
        </RadioGroup>
        {hasDelegation && (
          <p className="mt-2 text-xs text-muted-foreground">
            Removing delegation from a running engine isn&apos;t supported yet —
            recreate the engine to make it open. You can still switch between JWT
            and Impersonate.
          </p>
        )}
        {delegationMode === "jwt" && !jwtEnabled && (
          <p
            role="alert"
            className="mt-2 text-xs text-destructive"
          >
            JWT delegation needs “Accept egret JWT” enabled above (and a TLS
            coordinator, provisioned automatically). Without it, delegated queries
            will fail to authenticate.
          </p>
        )}
      </ConfigSection>

      {/* Ingress */}
      <ConfigSection
        title="Ingress exposure"
        description="Create a Kubernetes Ingress for the coordinator. TLS terminates at the ingress."
        docsUrl="https://kubernetes.io/docs/concepts/services-networking/ingress/"
      >
        <ConfigTable>
          <ConfigTableRow
            label="Expose via ingress"
            fieldKey="ingressEnabled"
            htmlFor="ced-ing-enabled"
          >
            <Switch
              id="ced-ing-enabled"
              checked={ingressEnabled}
              onCheckedChange={setIngressEnabled}
            />
          </ConfigTableRow>
          {ingressEnabled && (
            <>
              <ConfigTableRow
                label="Host"
                fieldKey="ingress.host"
                htmlFor="ced-ing-host"
              >
                <Input
                  id="ced-ing-host"
                  className="w-full"
                  value={ingressHost}
                  placeholder="trino-demo.example.com"
                  onChange={(e) => setIngressHost(e.target.value)}
                />
              </ConfigTableRow>
              <ConfigTableRow
                label="Ingress class"
                fieldKey="ingress.className"
                htmlFor="ced-ing-class"
              >
                <Input
                  id="ced-ing-class"
                  className="w-full"
                  value={ingressClassName}
                  placeholder="nginx"
                  onChange={(e) => setIngressClassName(e.target.value)}
                />
              </ConfigTableRow>
              <ConfigTableRow
                label="Ingress TLS cert"
                fieldKey="ingress.tlsCertSecretRef"
                htmlFor="es-ingress-tls-cert"
                details="Pick a kubernetes.io/tls secret to back the ingress cert, or keep the default."
              >
                <Select
                  value={ingressTlsCertSecretRef || "__none__"}
                  onValueChange={(v) =>
                    setIngressTlsCertSecretRef(v === "__none__" ? "" : v)
                  }
                >
                  <SelectTrigger id="es-ingress-tls-cert" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="__none__">None (default)</SelectItem>
                    {pickable
                      .filter((s) => s.type === "kubernetes.io/tls")
                      .map((s) => (
                        <SelectItem key={s.id} value={s.id}>
                          {s.name}
                        </SelectItem>
                      ))}
                  </SelectContent>
                </Select>
              </ConfigTableRow>
            </>
          )}
        </ConfigTable>
        {ingressEnabled && (
          <div className="space-y-4">
            <div className="space-y-1.5">
              <Label>Annotations</Label>
              <p className="text-xs text-muted-foreground">
                Extra ingress annotations (e.g. controller tuning).
              </p>
              <KeyValueEditor
                rows={ingressAnnotations}
                onChange={setIngressAnnotations}
                keyPlaceholder="nginx.ingress.kubernetes.io/proxy-read-timeout"
                valuePlaceholder="300"
                addLabel="Add annotation"
              />
            </div>
            {ingressHost.trim() && (
              <p className="text-xs text-muted-foreground">
                Clients connect to{" "}
                <code>https://{ingressHost.trim()}</code> with their
                username + password (or an egret JWT).
              </p>
            )}
          </div>
        )}
      </ConfigSection>
    </div>
  );
}
