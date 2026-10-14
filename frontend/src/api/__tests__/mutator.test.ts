import { afterEach, describe, expect, it, vi } from "vitest";
import { customInstance, rawBlob, rawText } from "@/api/mutator";

/** Every request's headers, as the stubbed `fetch` received them. */
function stubFetch() {
  const seen: Headers[] = [];
  vi.stubGlobal("fetch", vi.fn(async (_input: string, init?: RequestInit) => {
    seen.push(new Headers(init?.headers));
    return new Response("{}", { status: 200, headers: { "content-type": "application/json" } });
  }));
  return seen;
}

afterEach(() => vi.unstubAllGlobals());

describe("customInstance: the request's Content-Type", () => {
  it("says application/json for a string body that names no type (a route with two body types gets none from orval)", async () => {
    const seen = stubFetch();
    await customInstance("/api/ws/eng/workflows/flows/f/executions", { method: "POST", body: JSON.stringify({ inputs: {} }) });
    expect(seen[0].get("Content-Type")).toBe("application/json");
  });

  it("keeps a type the caller set", async () => {
    const seen = stubFetch();
    await customInstance("/api/ws/eng/kv", { method: "PUT", body: "raw", headers: { "Content-Type": "text/plain" } });
    expect(seen[0].get("Content-Type")).toBe("text/plain");
  });

  it("sets none for FormData or URLSearchParams, whose type (and boundary) the browser writes", async () => {
    const seen = stubFetch();
    const form = new FormData();
    form.set("f", new Blob(["x"]), "x.csv");
    await customInstance("/api/ws/eng/upload", { method: "POST", body: form });
    await customInstance("/api/token", { method: "POST", body: new URLSearchParams({ a: "1" }) });
    expect(seen[0].get("Content-Type")).toBeNull();
    expect(seen[1].get("Content-Type")).toBeNull();
  });

  it("sets none on a request with no body", async () => {
    const seen = stubFetch();
    await customInstance("/api/ws/eng/workflows/flows", { method: "GET" });
    expect(seen[0].get("Content-Type")).toBeNull();
  });
});

describe("customInstance: rawText", () => {
  const BODY = '{"type":"NUMBER","value":12345678901234567890.10}';
  function stubBody() {
    const inits: RequestInit[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_input: string, init?: RequestInit) => {
      inits.push(init ?? {});
      return new Response(BODY, { status: 200, headers: { "content-type": "application/json" } });
    }));
    return inits;
  }

  it("answers the body as its text, unparsed, so no digit is rounded", async () => {
    const inits = stubBody();
    const res = await customInstance<{ status: number; data: unknown }>("/api/ws/eng/kv/k", { method: "GET", rawText: true });
    expect(res.status).toBe(200);
    expect(rawText(res)).toBe(BODY);
    // The option is the mutator's: `fetch` never sees it.
    expect(inits[0]).not.toHaveProperty("rawText");
  });

  it("parses JSON as before without it", async () => {
    stubBody();
    const res = await customInstance<{ data: { value: number } }>("/api/ws/eng/kv/k", { method: "GET" });
    expect(res.data.value).toBe(12345678901234567000);
    expect(() => rawText(res)).toThrow(/rawText: true/);
  });
});

describe("customInstance: rawBlob", () => {
  // Bytes no UTF-8 decoder keeps: a PNG signature, then an invalid sequence and a NUL.
  const BYTES = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe, 0x00, 0x41]);
  function stubBytes() {
    const inits: RequestInit[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_input: string, init?: RequestInit) => {
      inits.push(init ?? {});
      return new Response(BYTES, { status: 200, headers: { "content-type": "application/octet-stream" } });
    }));
    return inits;
  }

  it("answers the body as a Blob holding every byte, never decoded as text", async () => {
    const inits = stubBytes();
    const res = await customInstance<{ status: number; data: unknown }>("/api/ws/eng/f", { method: "GET", rawBlob: true });
    expect(res.status).toBe(200);
    const bytes = new Uint8Array(await rawBlob(res).arrayBuffer());
    expect(Array.from(bytes)).toEqual(Array.from(BYTES));
    expect(inits[0]).not.toHaveProperty("rawBlob");
  });

  it("reads text as before without it", async () => {
    stubBytes();
    const res = await customInstance<{ data: unknown }>("/api/ws/eng/f", { method: "GET" });
    expect(typeof res.data).toBe("string");
    expect(() => rawBlob(res)).toThrow(/rawBlob: true/);
  });
});
