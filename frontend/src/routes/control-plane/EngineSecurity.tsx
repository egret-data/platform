import { useQuery } from "@tanstack/react-query";
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
    <div className="space-y-5 rounded-lg border p-4">
      <div className="space-y-0.5">
        <Label className="text-base">Exposure &amp; auth</Label>
        <p className="text-xs text-muted-foreground">
          Authenticate clients and expose the coordinator outside the
          cluster.
        </p>
      </div>

      {/* Password users */}
      <div className="space-y-2">
        <Label>Password users</Label>
        <p className="text-xs text-muted-foreground">
          Each user signs in with a username and the password held in
          the selected egret secret (hashed server-side).
        </p>
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
        </div>
      </div>

      {/* JWT */}
      <div className="space-y-3">
        <div className="flex items-center justify-between gap-4">
          <div className="space-y-0.5">
            <Label htmlFor="ced-jwt-enabled">Accept egret JWT</Label>
            <p className="text-xs text-muted-foreground">
              Also accept egret-issued JWTs (e.g. from notebooks) as
              client credentials.
            </p>
          </div>
          <Switch
            id="ced-jwt-enabled"
            checked={jwtEnabled}
            onCheckedChange={setJwtEnabled}
          />
        </div>
        {jwtEnabled && (
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <div className="space-y-1.5">
              <Label htmlFor="ced-jwt-jwks">JWKS URI</Label>
              <Input
                id="ced-jwt-jwks"
                value={jwksUri}
                placeholder="default IdP JWKS"
                onChange={(e) => setJwksUri(e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ced-jwt-aud">Required audience</Label>
              <Input
                id="ced-jwt-aud"
                value={requiredAudience}
                placeholder="optional"
                onChange={(e) => setRequiredAudience(e.target.value)}
              />
            </div>
          </div>
        )}
      </div>

      {/* Ingress */}
      <div className="space-y-3">
        <div className="flex items-center justify-between gap-4">
          <div className="space-y-0.5">
            <Label htmlFor="ced-ing-enabled">Expose via ingress</Label>
            <p className="text-xs text-muted-foreground">
              Create a Kubernetes Ingress for the coordinator. TLS
              terminates at the ingress.
            </p>
          </div>
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
                <Label htmlFor="ced-ing-host">Host</Label>
                <Input
                  id="ced-ing-host"
                  value={ingressHost}
                  placeholder="trino-demo.example.com"
                  onChange={(e) => setIngressHost(e.target.value)}
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="ced-ing-class">Ingress class</Label>
                <Input
                  id="ced-ing-class"
                  value={ingressClassName}
                  placeholder="nginx"
                  onChange={(e) => setIngressClassName(e.target.value)}
                />
              </div>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ced-ing-tls">TLS secret</Label>
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
      </div>
    </div>
  );
}
