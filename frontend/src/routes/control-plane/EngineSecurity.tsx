import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { Plus, Trash2 } from "lucide-react";

import { type Secret, listSecrets } from "@/api/controlPlane";
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
import { Switch } from "@/components/ui/switch";
import { ConfigSection } from "./config/ConfigSection";
import { FieldLabel } from "./config/FieldLabel";
import { type Kv, KeyValueEditor } from "./EngineResourceGroups";
import type { PasswordUserForm } from "./useEngineConfigForm";

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
  ingressAnnotations: Kv[];
  setIngressAnnotations: React.Dispatch<React.SetStateAction<Kv[]>>;
}) {
  const {
    passwordUsers,
    setPasswordUsers,
    jwtEnabled,
    setJwtEnabled,
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
    ingressTlsSecret,
    setIngressTlsSecret,
    ingressAnnotations,
    setIngressAnnotations,
  } = props;

  const secrets = useQuery<Secret[], ApiError>({
    queryKey: ["cp", "secrets"],
    queryFn: listSecrets,
    retry: false,
  });

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
                  {(secrets.data ?? []).length === 0 ? (
                    <SelectItem value="__none__" disabled>
                      No secrets
                    </SelectItem>
                  ) : (
                    (secrets.data ?? []).map((s) => (
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
          {(secrets.data ?? []).length === 0 && (
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
        <div className="flex items-center justify-between gap-4">
          <FieldLabel
            fieldKey="jwtEnabled"
            label="Accept egret JWT"
            htmlFor="ced-jwt-enabled"
          />
          <Switch
            id="ced-jwt-enabled"
            checked={jwtEnabled}
            onCheckedChange={setJwtEnabled}
          />
        </div>
        {jwtEnabled && (
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <div className="space-y-1.5">
              <FieldLabel
                fieldKey="jwksUri"
                label="JWKS URI"
                htmlFor="ced-jwt-jwks"
              />
              <Input
                id="ced-jwt-jwks"
                value={jwksUri}
                placeholder="default IdP JWKS"
                onChange={(e) => setJwksUri(e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <FieldLabel
                fieldKey="requiredAudience"
                label="Required audience"
                htmlFor="ced-jwt-aud"
              />
              <Input
                id="ced-jwt-aud"
                value={requiredAudience}
                placeholder="optional"
                onChange={(e) => setRequiredAudience(e.target.value)}
              />
            </div>
          </div>
        )}
      </ConfigSection>

      {/* Ingress */}
      <ConfigSection
        title="Ingress exposure"
        description="Create a Kubernetes Ingress for the coordinator. TLS terminates at the ingress."
        docsUrl="https://kubernetes.io/docs/concepts/services-networking/ingress/"
      >
        <div className="flex items-center justify-between gap-4">
          <FieldLabel
            fieldKey="ingressEnabled"
            label="Expose via ingress"
            htmlFor="ced-ing-enabled"
          />
          <Switch
            id="ced-ing-enabled"
            checked={ingressEnabled}
            onCheckedChange={setIngressEnabled}
          />
        </div>
        {ingressEnabled && (
          <div className="space-y-4">
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              <div className="space-y-1.5">
                <FieldLabel
                  fieldKey="ingress.host"
                  label="Host"
                  htmlFor="ced-ing-host"
                />
                <Input
                  id="ced-ing-host"
                  value={ingressHost}
                  placeholder="trino-demo.example.com"
                  onChange={(e) => setIngressHost(e.target.value)}
                />
              </div>
              <div className="space-y-1.5">
                <FieldLabel
                  fieldKey="ingress.className"
                  label="Ingress class"
                  htmlFor="ced-ing-class"
                />
                <Input
                  id="ced-ing-class"
                  value={ingressClassName}
                  placeholder="nginx"
                  onChange={(e) => setIngressClassName(e.target.value)}
                />
              </div>
            </div>
            <div className="space-y-1.5">
              <FieldLabel
                fieldKey="ingress.tlsSecret"
                label="TLS secret"
                htmlFor="ced-ing-tls"
              />
              <Input
                id="ced-ing-tls"
                value={ingressTlsSecret}
                placeholder="optional — Kubernetes TLS secret name"
                onChange={(e) => setIngressTlsSecret(e.target.value)}
              />
            </div>
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
