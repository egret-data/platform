import { ApiError } from "@/api/client";

/*
 * The Classification area's coded refusals (plan 2026-10-21 "Global
 * Constraints", error codes): the body is `{error, code, …fields}`, where
 * `error` keeps a full sentence and `code` is stable. Each narrower returns
 * the typed body when `e` is an `ApiError` carrying that code and an object
 * body, else `null`. The narrowers check the code, not the fields: the server
 * owns the shape, and these types follow it.
 */

/** A rule or policy named in a refusal; a hand-written (engine) policy has no rule id. */
export type NamedRule = { id: string | null; name: string };

/** `table_tag_mask_conflict` (400): a table-level classification would meet a mask on the same table. */
export type TableTagMaskConflict = {
  error: string;
  code: "table_tag_mask_conflict";
  tag: string;
  tagId: string;
  warehouseId: string | null;
  schema: string;
  table: string;
  mask: string;
  maskOrigin: "rule" | "policy";
  moreTables: boolean;
};

/** `rule_overlap` (409): two masks or row filters would decide one coordinate. */
export type RuleOverlapBody = {
  error: string;
  code: "rule_overlap";
  rules: NamedRule[];
  coordinate: string;
  kind: string;
};

/** `tag_in_use` (409): rules still use the classification's key. */
export type TagInUse = { error: string; code: "tag_in_use"; rules: { id: string; name: string }[] };

/** `tag_name_taken` (409): another classification has this name. */
export type TagNameTaken = {
  error: string;
  code: "tag_name_taken";
  name: string;
  existing: { id: string; key: string; name: string };
};

/** `tag_key_taken` (409): another classification has this key. */
export type TagKeyTaken = { error: string; code: "tag_key_taken"; key: string; existing: { id: string; name: string } };

/** `tag_key_invalid` (400): the key has a character outside `allowed` (`a-z 0-9 : -`). */
export type TagKeyInvalid = { error: string; code: "tag_key_invalid"; key: string; allowed: string };

function bodyOf<T>(e: unknown, code: string): T | null {
  if (!(e instanceof ApiError) || e.code !== code) return null;
  const body = e.body;
  if (typeof body !== "object" || body === null) return null;
  return body as T;
}

export const tableTagMaskConflict = (e: unknown) => bodyOf<TableTagMaskConflict>(e, "table_tag_mask_conflict");
export const ruleOverlap = (e: unknown) => bodyOf<RuleOverlapBody>(e, "rule_overlap");
export const tagInUse = (e: unknown) => bodyOf<TagInUse>(e, "tag_in_use");
export const tagNameTaken = (e: unknown) => bodyOf<TagNameTaken>(e, "tag_name_taken");
export const tagKeyTaken = (e: unknown) => bodyOf<TagKeyTaken>(e, "tag_key_taken");
export const tagKeyInvalid = (e: unknown) => bodyOf<TagKeyInvalid>(e, "tag_key_invalid");
