/**
 * Which classification the user set out to attach (Task 13: the steward job's
 * click count). A classification's "Attach in Data" records it; the next Add
 * classification dialog preselects it when it is offered there, and forgets
 * it once it is attached. In memory only (FE-DATA-3), one value, and emptied
 * on every identity switch (`clearUserScopedCache`). The user still sees the
 * choice and presses Add classification; nothing is attached for them.
 */
let intent: string | null = null;

export function setAttachIntent(tagId: string): void {
  intent = tagId;
}

export function attachIntent(): string | null {
  return intent;
}

export function clearAttachIntent(): void {
  intent = null;
}
