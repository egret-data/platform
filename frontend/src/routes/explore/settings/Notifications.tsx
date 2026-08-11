import { useEffect, useState } from "react";
import { useParams } from "react-router-dom";
import { AlertCircle, CheckCircle2, Mail } from "lucide-react";

import { useSmtpConfig, useSmtpMutations, type TlsMode } from "@/api/notifications";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { SettingsNote } from "@/routes/explore/settings/SettingsNote";

/**
 * Workspace SMTP settings.
 *
 * The password is the whole design problem here. The server never returns it,
 * so this form cannot render it — and must not pretend to, because a field
 * showing dots invites the reader to believe the real value is present and that
 * clearing it is how you keep it. Instead the field starts empty, a line of
 * text says whether one is stored, and an untouched field is simply omitted
 * from the save.
 */
export default function Notifications() {
  const ws = useParams().workspace!;
  const { data, isLoading } = useSmtpConfig(ws);
  const { save, sendTest, remove } = useSmtpMutations(ws);

  const [host, setHost] = useState("");
  const [port, setPort] = useState(587);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [fromAddress, setFromAddress] = useState("");
  const [fromName, setFromName] = useState("");
  const [tlsMode, setTlsMode] = useState<TlsMode>("starttls");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    if (!data) return;
    setHost(data.host);
    setPort(data.port);
    setUsername(data.username ?? "");
    setFromAddress(data.from_address);
    setFromName(data.from_name ?? "");
    setTlsMode(data.tls_mode);
    // Note: `password` is deliberately NOT set — there is nothing to set it to.
  }, [data]);

  const configured = Boolean(data);

  async function onSave(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setNotice(null);
    try {
      await save.mutateAsync({
        host,
        port,
        username: username || null,
        from_address: fromAddress,
        from_name: fromName || null,
        tls_mode: tlsMode,
        // Omitted entirely when untouched: the server reads a missing key as
        // "keep what you have" and an empty string as "remove it".
        ...(password ? { password } : {}),
      } as never);
      setPassword("");
      setNotice("Settings saved.");
    } catch (err) {
      setError(messageOf(err, "Could not save"));
    }
  }

  async function onTest() {
    setError(null);
    setNotice(null);
    try {
      await sendTest.mutateAsync(undefined);
      setNotice("Test message sent. Check the inbox for the from-address.");
    } catch (err) {
      setError(messageOf(err, "The relay refused the test message"));
    }
  }

  return (
    <div className="flex max-w-2xl flex-col gap-6 p-6">
      <header>
        <h2 className="flex items-center gap-2 text-lg font-semibold">
          <Mail className="h-5 w-5" />
          Notifications
        </h2>
        <SettingsNote>
          Alerts and reports in this workspace send mail through this relay, as this
          from-address. Each workspace has its own, so two teams never share an identity
          or a sending reputation.
        </SettingsNote>
      </header>

      {error && (
        <Alert variant="destructive">
          <AlertCircle className="h-4 w-4" />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      {notice && (
        <Alert>
          <CheckCircle2 className="h-4 w-4" />
          <AlertDescription>{notice}</AlertDescription>
        </Alert>
      )}

      <form onSubmit={onSave} className="flex flex-col gap-4">
        <div className="grid grid-cols-3 gap-3">
          <div className="col-span-2 flex flex-col gap-1.5">
            <Label htmlFor="smtp-host">SMTP host</Label>
            <Input
              id="smtp-host"
              value={host}
              onChange={(e) => setHost(e.target.value)}
              placeholder="smtp.example.com"
              required
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="smtp-port">Port</Label>
            <Input
              id="smtp-port"
              type="number"
              value={port}
              onChange={(e) => setPort(Number(e.target.value))}
              required
            />
          </div>
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="smtp-tls">Encryption</Label>
          <select
            id="smtp-tls"
            className="h-9 rounded-md border border-input bg-background px-3 text-sm"
            value={tlsMode}
            onChange={(e) => setTlsMode(e.target.value as TlsMode)}
          >
            <option value="starttls">STARTTLS (usually port 587)</option>
            <option value="implicit">TLS (usually port 465)</option>
            <option value="none">None — trusted internal relay only</option>
          </select>
          <p className="text-xs text-muted-foreground">
            Chosen explicitly rather than guessed from the port: guessing wrong can mean
            credentials sent in the clear.
          </p>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="smtp-username">Username</Label>
            <Input
              id="smtp-username"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              autoComplete="off"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="smtp-password">Password</Label>
            <Input
              id="smtp-password"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete="new-password"
              placeholder={data?.has_password ? "Unchanged" : ""}
            />
            {data?.has_password && (
              <p className="text-xs text-muted-foreground">
                A password is set. Leave this blank to keep it.
              </p>
            )}
          </div>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="smtp-from">From address</Label>
            <Input
              id="smtp-from"
              value={fromAddress}
              onChange={(e) => setFromAddress(e.target.value)}
              placeholder="alerts@example.com"
              required
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="smtp-from-name">From name</Label>
            <Input
              id="smtp-from-name"
              value={fromName}
              onChange={(e) => setFromName(e.target.value)}
              placeholder="Egret Alerts"
            />
          </div>
        </div>

        <div className="flex items-center gap-2">
          <Button type="submit" disabled={save.isPending || isLoading}>
            Save
          </Button>
          {/* Disabled until something is stored: the endpoint sends through the
              SAVED configuration, so testing unsaved edits would be misleading. */}
          <Button
            type="button"
            variant="secondary"
            onClick={onTest}
            disabled={!configured || sendTest.isPending}
          >
            Send test message
          </Button>
          {configured && (
            <Button
              type="button"
              variant="ghost"
              className="ml-auto text-destructive"
              onClick={async () => {
                await remove.mutateAsync();
                setNotice("Settings removed.");
              }}
            >
              Remove
            </Button>
          )}
        </div>
        <p className="text-xs text-muted-foreground">
          A test message is the only way to tell &ldquo;configured&rdquo; from
          &ldquo;configured correctly&rdquo; before an alert needs to fire.
        </p>
      </form>
    </div>
  );
}

/** Prefer the server's own message: it names the field or the relay's refusal. */
function messageOf(err: unknown, fallback: string): string {
  const body = (err as { body?: { error?: string } }).body;
  return body?.error ?? (err as Error)?.message ?? fallback;
}
