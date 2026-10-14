import { afterEach, describe, expect, it, vi } from "vitest";
import { customInstance, rawText } from "@/api/mutator";

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
