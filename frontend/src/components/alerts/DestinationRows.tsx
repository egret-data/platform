import { Mail, Plus, Trash2, Webhook, MessageSquare } from "lucide-react";

import { ConfigTableGroup, ConfigTableRow } from "@/components/config/ConfigTable";
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
 * Where a notification is delivered: a list you add to, each destination
 * carrying its own configuration.
 *
 * The shape follows Superset's "notification method" list, for the reason
 * Superset uses it — the channels are not a fixed set of fields, they are a
 * set the user composes. Two Slack channels, or a webhook per environment, are
 * ordinary requests that three fixed fields cannot express.
 *
 * Rendered as rows of the Notification tab's own `Setting | Details | Value`
 * table — a "Deliver to" group with one row per destination and a last row
 * that adds one — so the tab reads in one idiom rather than a card list above
 * a table (KI-220). The caller puts it inside a `ConfigTable`.
 *
 * A row is added CHOSEN rather than blank-then-typed, because the fields
 * differ per kind and an empty row with no kind has nothing to render.
 */
export function DestinationRows({
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
    <>
      <ConfigTableGroup label="Deliver to" />

      {value.map((d) => {
        const Icon = ICONS[d.kind];
        const removeButton = (
          <IconButton
            className="ml-auto hover:text-destructive"
            aria-label={`Remove ${DESTINATION_LABELS[d.kind]} destination`}
            onClick={() => remove(d.id)}
          >
            <Trash2 className="size-4" />
          </IconButton>
        );
        return (
          <ConfigTableRow
            key={d.id}
            label={
              <span className="inline-flex items-center gap-2">
                <Icon className="size-4 text-muted-foreground" aria-hidden />
                {DESTINATION_LABELS[d.kind]}
              </span>
            }
            details={
              d.kind === "email"
                ? "Comma-separated addresses."
                : d.kind === "slack"
                  ? "A channel the Slack app posts to."
                  : "Each POST is signed with HMAC-SHA256."
            }
            more={
              d.kind === "email"
                ? "Sending needs the workspace's SMTP relay, which is configured separately."
                : d.kind === "slack"
                  ? d.hasToken
                    ? "This destination still carries its own token, from before the workspace app existed. It keeps working; new ones use the workspace's."
                    : "Uses the workspace's Slack app, set in Settings → Notifications. The bot needs chat:write and an invite to the channel."
                  : "The receiver can verify with the signing secret that the call came from egret."
            }
          >
            {d.kind === "email" && (
              <div className="flex flex-col gap-1.5">
                <div className="flex items-center">
                  <Label htmlFor={`dest-${d.id}`}>Addresses</Label>
                  {removeButton}
                </div>
                <Input
                  id={`dest-${d.id}`}
                  value={d.addresses}
                  onChange={(e) => patch(d.id, { addresses: e.target.value })}
                  placeholder="on-call@example.com, team@example.com"
                />
              </div>
            )}

            {d.kind === "slack" && (
              <div className="flex flex-col gap-1.5">
                {/* No token field. The bot token belongs to the WORKSPACE, like
                    its SMTP relay — posting to three channels used to mean
                    pasting the same credential three times and rotating it in
                    three places. */}
                <div className="flex items-center">
                  <Label htmlFor={`dest-${d.id}`}>Channel</Label>
                  {removeButton}
                </div>
                <Input
                  id={`dest-${d.id}`}
                  value={d.channel}
                  onChange={(e) => patch(d.id, { channel: e.target.value })}
                  placeholder="#data-alerts"
                />
              </div>
            )}

            {d.kind === "webhook" && (
              <div className="flex flex-col gap-1.5">
                <div className="flex items-center">
                  <Label htmlFor={`dest-${d.id}`}>URL</Label>
                  {removeButton}
                </div>
                <Input
                  id={`dest-${d.id}`}
                  value={d.url}
                  onChange={(e) => patch(d.id, { url: e.target.value })}
                  placeholder="https://hooks.example.com/egret"
                />
                <Label htmlFor={`dest-${d.id}-secret`} className="mt-1">
                  Signing secret
                </Label>
                <Input
                  id={`dest-${d.id}-secret`}
                  type="password"
                  autoComplete="new-password"
                  value={d.secret}
                  disabled={d.clear}
                  onChange={(e) => patch(d.id, { secret: e.target.value })}
                  placeholder={d.hasSecret ? "Unchanged" : "Optional"}
                />
                {/* Per-row is what makes this settable at all: one secret
                    field above a textarea of several URLs could not say WHICH
                    URL it signed, so the editor could only ever keep an
                    existing secret, never set one. */}
                {d.hasSecret && !d.clear && (
                  <button
                    type="button"
                    className="self-start text-xs text-muted-foreground underline hover:text-foreground"
                    onClick={() => patch(d.id, { clear: true, secret: "" })}
                  >
                    Remove the stored secret
                  </button>
                )}
                {d.clear && (
                  <p className="text-xs text-destructive">
                    The stored secret will be removed on save.{" "}
                    <button
                      type="button"
                      className="underline"
                      onClick={() => patch(d.id, { clear: false })}
                    >
                      Undo
                    </button>
                  </p>
                )}
              </div>
            )}
          </ConfigTableRow>
        );
      })}

      <ConfigTableRow
        label="Add a destination"
        details={
          value.length === 0
            ? "None yet: runs are recorded, but nobody is told."
            : "Email, Slack or a webhook, as many of each as you need."
        }
      >
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button type="button" variant="outline" size="sm">
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
      </ConfigTableRow>
    </>
  );
}
