/**
 * Turns a ticket-request failure into something a person can read, shared by the editor
 * (`AppEditPage`) and the viewer (`AppViewPage`) — the ticket-lifecycle logic is identical
 * between them; only the wording and one extra 409 case differ by `mode`.
 */
export function describeFailure(err: unknown, mode: "edit" | "run" = "edit"): { title: string; message: string } {
  const status = (err as { status?: number })?.status;
  const detail = (err as Error)?.message;
  const verb = mode === "edit" ? "edit" : "open";
  switch (status) {
    case 403:
      return {
        title: `You can't ${verb} this app`,
        message:
          mode === "edit"
            ? "Editing needs permission to write this app and to run queries in this workspace. Ask a workspace admin."
            : "Viewing needs permission to run queries in this workspace. Ask a workspace admin.",
      };
    case 404:
      return { title: "App not found", message: "It may have been deleted, or it belongs to another workspace." };
    case 409:
      // Run-only: the author has never published a version, distinct from a trashed app (both 409).
      // Our own sentence, not the server's lower-case detail under the title.
      if (mode === "run" && /published version/i.test(detail ?? "")) {
        return {
          title: "Nobody has published this app yet",
          message: "There is no version to open. Ask the author to publish one, then open the app again.",
        };
      }
      return { title: "This app can't be opened right now", message: detail || "It is in the trash." };
    case 503:
      return {
        title: "Apps are not set up on this deployment",
        message: "The apps origin is not configured. An operator needs to set apps.origin.",
      };
    default:
      return {
        title: `Couldn't open the ${mode === "edit" ? "editor" : "app"}`,
        message: detail || "Something went wrong asking for the app.",
      };
  }
}
