import { Mail, Plus, Trash2, Webhook, MessageSquare } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu";
import { IconButton } from "@/components/ui/icon-button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  DESTINATION_LABELS,
  emptyDestination,
  type Destination,
  type DestinationKind,
} from "./destinations";

const ICONS: Record<DestinationKind, typeof Mail> = {
  email: Mail,
  slack: MessageSquare,
  webhook: Webhook,
};

const KINDS: DestinationKind[] = ["email", "slack", "webhook"];

/**
 * Where a notification is delivered: a list you add to, each row carrying its
 * own configuration.
 *
 * The shape follows Superset's "notification method" list, for the reason
 * Superset uses it — the channels are not a fixed set of fields, they are a
 * set the user composes. Two Slack channels, or a webhook per environment, are
 * ordinary requests that three fixed fields cannot express.
 *
 * A row is added CHOSEN rather than blank-then-typed, because the fields
 * differ per kind and an empty row with no kind has nothing to render.
 */
export function DestinationList({
  value,
  onChange,
}: {
  value: Destination[];
  onChange: (next: Destination[]) => void;
}) {
  const patch = (id: string, fields: Partial<Destination>) =>
    onChange(value.map((d) => (d.id === id ? ({ ...d, ...fields } as Destination) : d)));
  const remove = (id: string) => onChange(value.filter((d) => d.id !== id));

  return (
    <div className="flex flex-col gap-2">
      <Label>Deliver to</Label>
      {value.length === 0 && (
        <p className="rounded-md border border-dashed px-3 py-6 text-center text-sm text-muted-foreground">
          No destinations yet. A schedule with none runs and records its result, but tells nobody.
        </p>
      )}

      {value.map((d) => {
        const Icon = ICONS[d.kind];
        return (
          <div key={d.id} className="rounded-md border p-3">
            <div className="mb-2 flex items-center gap-2">
              <Icon className="size-4 text-muted-foreground" aria-hidden />
              <span className="text-sm font-medium">{DESTINATION_LABELS[d.kind]}</span>
              <IconButton
                className="ml-auto hover:text-destructive"
                aria-label={`Remove ${DESTINATION_LABELS[d.kind]} destination`}
                onClick={() => remove(d.id)}
              >
                <Trash2 className="size-4" />
              </IconButton>
            </div>

            {d.kind === "email" && (
              <div className="flex flex-col gap-1.5">
                <Label htmlFor={`dest-${d.id}`}>Addresses</Label>
                <Input
                  id={`dest-${d.id}`}
                  value={d.addresses}
                  onChange={(e) => patch(d.id, { addresses: e.target.value })}
                  placeholder="on-call@example.com, team@example.com"
                />
                <p className="text-xs text-muted-foreground">
                  Comma separated. Sending needs workspace SMTP, which is configured separately.
                </p>
              </div>
            )}

            {d.kind === "slack" && (
              <div className="flex flex-col gap-1.5">
                <Label htmlFor={`dest-${d.id}`}>Channel</Label>
                <Input
                  id={`dest-${d.id}`}
                  value={d.channel}
                  onChange={(e) => patch(d.id, { channel: e.target.value })}
                  placeholder="#data-alerts"
                />
                {/* No token field. The bot token belongs to the WORKSPACE, like
                    its SMTP relay — posting to three channels used to mean
                    pasting the same credential three times and rotating it in
                    three places. */}
                <p className="text-xs text-muted-foreground">
                  {d.hasToken
                    ? "This destination still carries its own token, from before the workspace app existed. It keeps working; new ones use the workspace's."
                    : "Uses the workspace's Slack app, set in Settings → Notifications. The bot needs chat:write and an invite to the channel."}
                </p>
              </div>
            )}

            {d.kind === "webhook" && (
              <div className="flex flex-col gap-1.5">
                <div className="flex gap-2">
                  <div className="flex flex-1 flex-col gap-1.5">
                    <Label htmlFor={`dest-${d.id}`}>URL</Label>
                    <Input
                      id={`dest-${d.id}`}
                      value={d.url}
                      onChange={(e) => patch(d.id, { url: e.target.value })}
                      placeholder="https://hooks.example.com/egret"
                    />
                  </div>
                  <div className="flex flex-1 flex-col gap-1.5">
                    <Label htmlFor={`dest-${d.id}-secret`}>Signing secret</Label>
                    <Input
                      id={`dest-${d.id}-secret`}
                      type="password"
                      autoComplete="new-password"
                      value={d.secret}
                      disabled={d.clear}
                      onChange={(e) => patch(d.id, { secret: e.target.value })}
                      placeholder={d.hasSecret ? "Unchanged" : "Optional"}
                    />
                  </div>
                </div>
                <p className="text-xs text-muted-foreground">
                  {/* Per-row is what makes this settable at all: one secret
                      field above a textarea of several URLs could not say
                      WHICH URL it signed, so the editor could only ever keep
                      an existing secret, never set one. */}
                  Each POST is signed with HMAC-SHA256 so the receiver can verify it came from
                  egret.{" "}
                  {d.hasSecret && !d.clear && (
                    <button
                      type="button"
                      className="underline hover:text-foreground"
                      onClick={() => patch(d.id, { clear: true, secret: "" })}
                    >
                      Remove the stored secret
                    </button>
                  )}
                  {d.clear && (
                    <span className="text-destructive">
                      The stored secret will be removed on save.{" "}
                      <button
                        type="button"
                        className="underline"
                        onClick={() => patch(d.id, { clear: false })}
                      >
                        Undo
                      </button>
                    </span>
                  )}
                </p>
              </div>
            )}
          </div>
        );
      })}

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button type="button" variant="outline" size="sm" className="self-start">
            <Plus className="mr-1.5 size-4" />
            Add delivery method
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          {KINDS.map((kind) => {
            const Icon = ICONS[kind];
            return (
              <DropdownMenuItem key={kind} onClick={() => onChange([...value, emptyDestination(kind)])}>
                <Icon className="mr-2 size-4" aria-hidden />
                {DESTINATION_LABELS[kind]}
              </DropdownMenuItem>
            );
          })}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
