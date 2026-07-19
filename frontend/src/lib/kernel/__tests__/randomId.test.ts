import { describe, it, expect, afterEach } from "vitest";
import { randomId } from "../protocol";

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

describe("randomId", () => {
  const realRandomUUID = globalThis.crypto?.randomUUID;
  afterEach(() => {
    if (globalThis.crypto) {
      // restore
      (globalThis.crypto as unknown as { randomUUID?: unknown }).randomUUID = realRandomUUID;
    }
  });

  it("returns a v4 uuid", () => {
    expect(randomId()).toMatch(UUID_RE);
  });

  it("still works in a non-secure context where crypto.randomUUID is undefined", () => {
    // Simulate http/non-localhost: randomUUID missing, getRandomValues present.
    (globalThis.crypto as unknown as { randomUUID?: unknown }).randomUUID = undefined;
    const id = randomId();
    expect(id).toMatch(UUID_RE);
    expect(randomId()).not.toBe(id); // distinct
  });
});
