import { describe, expect, it } from "vitest";

import { emptyDestination, toDestinations, toRecipients } from "../destinations";

describe("delivery destinations", () => {
  it("round-trips several destinations of the same kind", () => {
    // What the flat form could not express: the old editor had ONE Slack
    // field, so a schedule posting to two channels could be created by the
    // API and then silently reduced to one by opening and saving it.
    const rows = toDestinations([
      { id: "r1", kind: "slack", config: { channel: "#data", has_token: true } },
      { id: "r2", kind: "slack", config: { channel: "#exec", has_token: false } },
    ]);
    expect(rows).toHaveLength(2);
    expect(toRecipients(rows)).toEqual([
      { kind: "slack", config: { channel: "#data" } },
      { kind: "slack", config: { channel: "#exec" } },
    ]);
  });

  it("gives every row a distinct id", () => {
    // The remove button and React's key both address rows by id. Two rows
    // sharing one would remove the wrong destination.
    const rows = toDestinations([
      { id: "r1", kind: "email", config: { addresses: ["a@t.local"] } },
      { id: "r2", kind: "email", config: { addresses: ["b@t.local"] } },
    ]);
    const added = [...rows, emptyDestination("email")];
    expect(new Set(added.map((d) => d.id)).size).toBe(added.length);
  });

  it("never carries a credential back to the server unasked", () => {
    // `has_secret`/`has_token` are REPORTS, not inputs. An absent key means
    // "keep the stored one" (`ScheduleRepo::vault_secrets`), so an untouched
    // row must send the destination alone.
    const rows = toDestinations([
      { id: "r1", kind: "webhook", config: { url: "https://h.example/1", has_secret: true } },
      { id: "r2", kind: "slack", config: { channel: "#data", has_token: true } },
    ]);
    expect(toRecipients(rows)).toEqual([
      { kind: "webhook", config: { url: "https://h.example/1" } },
      { kind: "slack", config: { channel: "#data" } },
    ]);
  });

  it("sends a typed secret for its own row and nothing for the others", () => {
    // The reason this is settable at all: with one secret field above a
    // textarea of URLs there was no way to say WHICH url it signed, so the
    // old editor could only keep a secret, never set one.
    const rows = toDestinations([
      { id: "r1", kind: "webhook", config: { url: "https://h.example/1" } },
      { id: "r2", kind: "webhook", config: { url: "https://h.example/2" } },
    ]);
    rows[1] = { ...rows[1], secret: "s3cret" } as typeof rows[1];
    expect(toRecipients(rows)).toEqual([
      { kind: "webhook", config: { url: "https://h.example/1" } },
      { kind: "webhook", config: { url: "https://h.example/2", secret: "s3cret" } },
    ]);
  });

  it("removes a stored secret only when that is asked for explicitly", () => {
    // An empty field means "keep" — the editor cannot render the stored value,
    // so blank cannot mean delete. `""` is what the server reads as remove,
    // and only the Remove control produces it.
    const [row] = toDestinations([
      { id: "r1", kind: "webhook", config: { url: "https://h.example/1", has_secret: true } },
    ]);
    expect(toRecipients([row])[0].config).toEqual({ url: "https://h.example/1" });
    expect(toRecipients([{ ...row, clear: true } as typeof row])[0].config).toEqual({
      url: "https://h.example/1",
      secret: "",
    });
  });

  it("drops a row the user left empty rather than sending a blank destination", () => {
    // Adding a method and then not filling it in is a half-finished thought,
    // not a request to create a recipient with no address.
    expect(toRecipients([emptyDestination("email"), emptyDestination("webhook")])).toEqual([]);
  });

  it("ignores a recipient kind this build does not know", () => {
    // Forward compatibility in the direction that matters: an editor that
    // rendered nothing for an unknown kind would still DELETE it on save,
    // because the update replaces the whole set. Dropping it from the rows is
    // the same outcome, but the code says so.
    // Cast because the generated DTO's `kind` is a closed union — which is
    // exactly why the runtime guard is worth a test: types describe the
    // server this client was BUILT against, not the one it is talking to.
    const future = [{ id: "r1", kind: "pigeon", config: { loft: "n1" } }] as unknown as Parameters<
      typeof toDestinations
    >[0];
    expect(toDestinations(future)).toEqual([]);
  });
});
