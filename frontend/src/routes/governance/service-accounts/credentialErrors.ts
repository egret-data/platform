import { ApiError } from "@/api/client";

/**
 * What a refused credential call says. The server's own sentence wins
 * (FE-ACTION-3); a bare status gets a plain explanation: 403 (the caller may
 * not mint this audience), 409 (the credential is already revoked) and 400
 * (for example a Data API credential on a platform account).
 */
export function credentialErrorText(e: unknown): string {
  if (e instanceof ApiError) {
    const bare = ["Forbidden", "Conflict", "Bad Request", ""].includes(e.message);
    if (!bare) return e.message;
    if (e.status === 403) return "You don't have permission to do that for this audience.";
    if (e.status === 409) return "That credential is already revoked, so it can't be changed.";
    if (e.status === 400) return "The server refused the request. Check the audience and the account's scope.";
  }
  return e instanceof Error && e.message ? e.message : "The change could not be saved. Try again.";
}

/** The generated client types a body as the success shape or an error body; only the success shape is one. */
export function asCreated<T extends object>(body: T | { error?: unknown }): T {
  if (!("client_secret" in body)) throw new Error("The server did not return the credential.");
  return body as T;
}

export function asList<T>(body: T[] | object): T[] {
  if (!Array.isArray(body)) throw new Error("The credentials could not be loaded.");
  return body as T[];
}
