/** Helpers that keep a stored value visible in the editor's pickers (KI-270, KI-277). */

export interface PickerOption {
  value: string;
  label: string;
}

/**
 * The options a picker shows: its own items, plus the stored value as an
 * explicit, marked option when no item carries it. A Select whose value matches
 * no item renders an empty box, so a value saved through the API (`csv`) or a
 * chart that has since gone would read as "nothing chosen".
 */
export function withStoredOption(
  items: PickerOption[],
  stored: string,
  missingLabel: (stored: string) => string,
): PickerOption[] {
  if (!stored || items.some((i) => i.value === stored)) return items;
  return [...items, { value: stored, label: missingLabel(stored) }];
}

/**
 * A stored owner's display name: the workspace's user member when there is
 * one, else the email the schedule itself carries (a group-only or former
 * member is not in the user-member list), else the raw id.
 */
export function ownerLabel(
  id: string,
  members: { principal_id: string; principal_name?: string | null }[],
  owners: { user_id: string; email: string }[] | undefined,
): string {
  return (
    members.find((m) => m.principal_id === id)?.principal_name ??
    owners?.find((o) => o.user_id === id)?.email ??
    id
  );
}
