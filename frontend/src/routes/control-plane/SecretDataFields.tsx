import { forwardRef, useImperativeHandle, useState } from "react";
import { Plus, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";

/** Selectable k8s-style secret types. */
export const SECRET_TYPES = [
  "Opaque",
  "kubernetes.io/tls",
  "kubernetes.io/basic-auth",
  "kubernetes.io/dockerconfigjson",
  "kubernetes.io/ssh-auth",
] as const;

interface OpaqueRow {
  key: string;
  value: string;
}

export interface SecretDataHandle {
  /** Build the `data` record from the current inputs (write-only; never prefilled). */
  getData: () => Record<string, string>;
}

/**
 * The per-type value inputs for a secret (Opaque key/value rows, TLS, basic-auth,
 * dockerconfigjson, ssh-auth). Owns its own write-only state and exposes the
 * assembled `data` map via a ref, so it can back both the create and rotate
 * flows. Remount (via a `key`) to clear it.
 */
export const SecretDataFields = forwardRef<SecretDataHandle, { type: string }>(
  function SecretDataFields({ type }, ref) {
    const [rows, setRows] = useState<OpaqueRow[]>([{ key: "value", value: "" }]);
    const [tlsCrt, setTlsCrt] = useState("");
    const [tlsKey, setTlsKey] = useState("");
    const [username, setUsername] = useState("");
    const [password, setPassword] = useState("");
    const [dockerRegistry, setDockerRegistry] = useState("");
    const [dockerUsername, setDockerUsername] = useState("");
    const [dockerPassword, setDockerPassword] = useState("");
    const [dockerEmail, setDockerEmail] = useState("");
    const [sshKey, setSshKey] = useState("");

    useImperativeHandle(ref, () => ({
      getData: () => {
        if (type === "kubernetes.io/tls") {
          return { "tls.crt": tlsCrt, "tls.key": tlsKey };
        }
        if (type === "kubernetes.io/basic-auth") {
          return { username, password };
        }
        if (type === "kubernetes.io/dockerconfigjson") {
          const registry = dockerRegistry.trim();
          const auth = btoa(`${dockerUsername}:${dockerPassword}`);
          const dockercfg = JSON.stringify({
            auths: {
              [registry]: {
                username: dockerUsername,
                password: dockerPassword,
                email: dockerEmail.trim() || undefined,
                auth,
              },
            },
          });
          return { ".dockerconfigjson": dockercfg };
        }
        if (type === "kubernetes.io/ssh-auth") {
          return { "ssh-privatekey": sshKey };
        }
        // Opaque
        return Object.fromEntries(
          rows.filter((r) => r.key.trim().length > 0).map((r) => [r.key.trim(), r.value]),
        );
      },
    }));

    if (type === "Opaque") {
      return (
        <div className="space-y-2">
          <Label>Data</Label>
          {rows.map((row, i) => (
            <div key={i} className="flex items-center gap-2">
              <Input
                aria-label={`Key ${i + 1}`}
                value={row.key}
                onChange={(e) =>
                  setRows((rs) => rs.map((r, j) => (j === i ? { ...r, key: e.target.value } : r)))
                }
                placeholder="key"
                className="flex-1"
              />
              <Input
                aria-label={`Value ${i + 1}`}
                type="password"
                value={row.value}
                onChange={(e) =>
                  setRows((rs) => rs.map((r, j) => (j === i ? { ...r, value: e.target.value } : r)))
                }
                placeholder="value"
                autoComplete="new-password"
                className="flex-1"
              />
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="text-muted-foreground hover:text-destructive"
                onClick={() =>
                  setRows((rs) => (rs.length > 1 ? rs.filter((_, j) => j !== i) : rs))
                }
                disabled={rows.length <= 1}
                aria-label={`Remove row ${i + 1}`}
              >
                <X className="h-4 w-4" />
              </Button>
            </div>
          ))}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setRows((rs) => [...rs, { key: "", value: "" }])}
          >
            <Plus className="h-4 w-4" />
            Add key
          </Button>
        </div>
      );
    }

    if (type === "kubernetes.io/tls") {
      return (
        <>
          <div className="space-y-1.5">
            <Label htmlFor="secret-tls-crt">tls.crt</Label>
            <Textarea
              id="secret-tls-crt"
              value={tlsCrt}
              onChange={(e) => setTlsCrt(e.target.value)}
              placeholder="-----BEGIN CERTIFICATE-----"
              className="font-mono text-xs"
              rows={4}
              required
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="secret-tls-key">tls.key</Label>
            <Textarea
              id="secret-tls-key"
              value={tlsKey}
              onChange={(e) => setTlsKey(e.target.value)}
              placeholder="-----BEGIN PRIVATE KEY-----"
              className="font-mono text-xs"
              rows={4}
              required
            />
          </div>
        </>
      );
    }

    if (type === "kubernetes.io/basic-auth") {
      return (
        <>
          <div className="space-y-1.5">
            <Label htmlFor="secret-username">Username</Label>
            <Input
              id="secret-username"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder="Username"
              required
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="secret-password">Password</Label>
            <Input
              id="secret-password"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="Password"
              autoComplete="new-password"
              required
            />
          </div>
        </>
      );
    }

    if (type === "kubernetes.io/dockerconfigjson") {
      return (
        <>
          <div className="space-y-1.5">
            <Label htmlFor="secret-docker-registry">Registry</Label>
            <Input
              id="secret-docker-registry"
              value={dockerRegistry}
              onChange={(e) => setDockerRegistry(e.target.value)}
              placeholder="registry.example.com"
              required
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="secret-docker-username">Username</Label>
            <Input
              id="secret-docker-username"
              value={dockerUsername}
              onChange={(e) => setDockerUsername(e.target.value)}
              placeholder="Username"
              required
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="secret-docker-password">Password</Label>
            <Input
              id="secret-docker-password"
              type="password"
              value={dockerPassword}
              onChange={(e) => setDockerPassword(e.target.value)}
              placeholder="Password"
              autoComplete="new-password"
              required
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="secret-docker-email">Email</Label>
            <Input
              id="secret-docker-email"
              type="email"
              value={dockerEmail}
              onChange={(e) => setDockerEmail(e.target.value)}
              placeholder="Optional"
            />
          </div>
        </>
      );
    }

    if (type === "kubernetes.io/ssh-auth") {
      return (
        <div className="space-y-1.5">
          <Label htmlFor="secret-ssh-key">ssh-privatekey</Label>
          <Textarea
            id="secret-ssh-key"
            value={sshKey}
            onChange={(e) => setSshKey(e.target.value)}
            placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
            className="font-mono text-xs"
            rows={4}
            required
          />
        </div>
      );
    }

    return null;
  },
);
