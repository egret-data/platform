import type { Recipient } from "@/api/alerts";

/** A recipient as the write body carries it: the kind, and its own config. */
type RecipientInput = { kind: "email" | "slack" | "webhook"; config: Record<string, unknown> };

/**
 * One place a notification is delivered to, as the editor holds it.
 *
 * The server has always stored `recipients` as a list of `{kind, config}` and
 * the runner has always looped over all of them — several webhooks, several
 * Slack channels and several email groups all work. Only the FORM flattened
 * that into "one email field, one Slack field, one webhook textarea", which is
 * why adding a second Slack channel was impossible through the UI and why the
 * webhook signing secret had nowhere to live (one field, several URLs).
 *
 * Modelling a destination as a row with its own configuration removes both
 * limits, and matches how Superset's "notification method" list reads.
 */
export type Destination =
  | { id: string; kind: "email"; addresses: string }
  | { id: string; kind: "slack"; channel: string; hasToken: boolean }
  | { id: string; kind: "webhook"; url: string; secret: string; hasSecret: boolean; clear: boolean };

export type DestinationKind = Destination["kind"];

export const DESTINATION_LABELS: Record<DestinationKind, string> = {
  email: "Email",
  slack: "Slack",
  webhook: "Webhook",
};

/**
 * Row identity for React's key and for the remove button.
 *
 * A monotonic counter, not the array index: removing the first of three rows
 * would otherwise renumber the survivors and hand React a key it already used,
 * which remounts inputs and moves the caret while someone is typing. Not the
 * URL or channel either — those are the fields being edited, so they change
 * under the key.
 */
let seq = 0;
export const nextDestinationId = () => `d${(seq += 1)}`;

export function emptyDestination(kind: DestinationKind): Destination {
  const id = nextDestinationId();
  switch (kind) {
    case "email":
      return { id, kind, addresses: "" };
    case "slack":
      return { id, kind, channel: "", hasToken: false };
    case "webhook":
      return { id, kind, url: "", secret: "", hasSecret: false, clear: false };
  }
}

/**
 * Read the server's recipient list into editor rows, preserving order.
 *
 * A credential is never in the response — the DTO reports only `has_secret` /
 * `has_token` — so the row records that one EXISTS and leaves the input empty.
 */
export function toDestinations(recipients: Recipient[]): Destination[] {
  return recipients.flatMap((r): Destination[] => {
    const c = (r.config ?? {}) as Record<string, unknown>;
    const id = nextDestinationId();
    if (r.kind === "email") {
      const addresses = Array.isArray(c.addresses) ? (c.addresses as string[]) : [];
      return [{ id, kind: "email", addresses: addresses.join(", ") }];
    }
    if (r.kind === "slack") {
      return [
        {
          id,
          kind: "slack",
          channel: typeof c.channel === "string" ? c.channel : "",
          // Reported, not edited: a destination predating the workspace app
          // still carries its own token, and the row says so.
          hasToken: c.has_token === true,
        },
      ];
    }
    if (r.kind === "webhook") {
      return [
        {
          id,
          kind: "webhook",
          url: typeof c.url === "string" ? c.url : "",
          secret: "",
          hasSecret: c.has_secret === true,
          clear: false,
        },
      ];
    }
    // A kind this build does not know — drop it rather than render an empty
    // row the user cannot edit but would silently delete on save.
    return [];
  });
}

/**
 * Editor rows back into the write body's `recipients`.
 *
 * The three-way credential rule (`ScheduleRepo::vault_secrets`) is what the
 * `secret`/`token` keys encode, and it is why this cannot simply always send
 * the field:
 *
 * - key absent  → keep whatever is stored for that URL/channel;
 * - `""`        → remove it;
 * - a value     → replace it.
 *
 * So an untouched row sends no credential at all and the browser never has to
 * hold one in order to avoid destroying it.
 */
export function toRecipients(destinations: Destination[]): RecipientInput[] {
  return destinations.flatMap((d): RecipientInput[] => {
    if (d.kind === "email") {
      const addresses = d.addresses
        .split(",")
        .map((a) => a.trim())
        .filter(Boolean);
      return addresses.length ? [{ kind: "email" as const, config: { addresses } }] : [];
    }
    if (d.kind === "slack") {
      const channel = d.channel.trim();
      if (!channel) return [];
      // Channel only. The token is the WORKSPACE's now (Settings →
      // Notifications), and an absent `token` key means "keep whatever this
      // recipient already has" server-side — which is what preserves a
      // pre-workspace-app destination's own credential untouched.
      return [{ kind: "slack" as const, config: { channel } }];
    }
    const url = d.url.trim();
    if (!url) return [];
    const secret = d.clear ? "" : d.secret.trim();
    return [
      {
        kind: "webhook" as const,
        config: { url, ...(d.clear || secret ? { secret } : {}) },
      },
    ];
  });
}
