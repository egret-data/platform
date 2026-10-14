/**
 * Why every KV write is off for a viewer without `workflow:manage` (W7d spec
 * §4.5, FE-ACTION-4). One sentence, so every disabled control says the same.
 */
export const MANAGE_REASON = "Changing KV keys needs the workflow:manage permission";
