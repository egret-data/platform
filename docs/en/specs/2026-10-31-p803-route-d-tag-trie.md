# #803 — Route D lists what Ranger's tag trie applies

## Problem

Route D (`GET /governance/classifications/{id}/handwritten`, the classification's
**Manual policies** tab) listed a hand-written tag policy when the policy
*matcher* accepted the tag name. Ranger's tag phase never calls the matcher for
a tag request: the tag repository's resource trie is the whole filter, and it
is wider. Measured end-to-end through `RangerPolicyEngineImpl` (2.5.0, 2.8.0,
2.9.0, identical): 888 (shape × tag) pairs, 305 that Route D omitted, 0 that it
listed wrongly. A steward on `phone` did not see a deny written as `*i` that the
engines apply to `phone`.

## The rule (reproduces all 888 pairs)

Per value of the policy's `tag` level, `prefix` = the value up to its first
`*`, `?` or `{`:

- `isExcludes` → every tag;
- the value has a wildcard/token, or the level `isRecursive` → every tag starting
  with `prefix` (case-sensitive);
- otherwise → the tag equal to the value.

## Change

- `egret_ranger_eval::resource::tag_trie_candidacy(policy, def, tag)` →
  `NotReturned | Exact | Prefix(p) | NotMeasured`.
- `egret_perm_compile::handwritten::tag_reach(policy, def, tag)` — Route D's
  per-policy decision: the trie decides; when the trie returns the policy but
  the matcher rejects the tag, the row carries `widenedByEngine: true` and
  `engineTagPrefix` (`""` = every tag).
- Manual policies tab: a `Wider than its pattern` status badge and the sentence
  "The engine applies this policy to every tag starting with `p`, not only the
  tags its pattern matches." (or "to every tag").
- Explain is unchanged (it already refuses these shapes).

## Decisions taken

- **Unmeasured shapes keep the matcher's answer** (old behaviour): a resource
  map other than exactly `{tag}`, additional resource maps, an empty value list
  or an empty value, and a `tag` resource def whose matcher or options differ
  from the bundled tag service-def. Conservative: no guessed widening.
- Several values reaching a tag: the row states the **longest** prefix (the most
  specific reason); an exact literal match wins over any prefix.
- No migration.

## Evidence

`crates/egret-perm-compile/tests/route_d_tag_pattern.rs` reads the committed
corpus `tests/fixtures/route_d_tag_trie.json` (37 shapes × 24 tags + Ranger's
verdicts) and requires `tag_reach` to equal Ranger on all 888 pairs, for access
and masking policies. On the old (matcher-only) decision it fails with 305
wrong pairs per policy type.
