import type { QueryClient } from "@tanstack/react-query";

/** `GET /perm/rules/enforcement` and `GET /perm/rules/{id}/effect`, any params. */
const RULE_ENFORCEMENT_KEY = /^\/api\/control-plane\/perm\/rules\/(enforcement|[^/]+\/effect)$/;

/**
 * What changes a stored rule's enforcement or effect: a rule write, a
 * classification attached or removed (what a classification rule reaches), an
 * engine attached. Every such answer is refetched. A module of its own, with
 * no imports beyond a type, so `api/tags.ts` can call it without pulling the
 * rule UI into every bundle that reads tags.
 */
export function invalidateRuleEnforcement(queryClient: QueryClient) {
  void queryClient.invalidateQueries({
    predicate: (q) => typeof q.queryKey[0] === "string" && RULE_ENFORCEMENT_KEY.test(q.queryKey[0]),
  });
}
