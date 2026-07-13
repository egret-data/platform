import { toast } from "sonner";

export interface BulkResult<T> {
  ok: T[];
  failed: T[];
}

/**
 * Run `op` over each item independently (collecting successes + failures), refresh,
 * then toast a summary. When `undo` is provided and at least one item succeeded, the
 * toast offers an Undo that runs `undo` over only the SUCCEEDED items.
 */
export async function runBulk<T>(opts: {
  items: T[];
  op: (item: T) => Promise<unknown>;
  verb: string;
  noun: string;
  refresh: () => void;
  undo?: (item: T) => Promise<unknown>;
}): Promise<BulkResult<T>> {
  const ok: T[] = [];
  const failed: T[] = [];
  for (const item of opts.items) {
    try {
      await opts.op(item);
      ok.push(item);
    } catch {
      failed.push(item);
    }
  }
  opts.refresh();

  const text =
    failed.length === 0
      ? `${opts.verb} ${ok.length} ${opts.noun}.`
      : `${opts.verb} ${ok.length} of ${opts.items.length} ${opts.noun}; ${failed.length} failed.`;

  const action =
    opts.undo && ok.length > 0
      ? {
          label: "Undo",
          onClick: async () => {
            for (const item of ok) {
              try {
                await opts.undo!(item);
              } catch {
                /* best-effort undo */
              }
            }
            opts.refresh();
            toast.success("Undone");
          },
        }
      : undefined;

  if (failed.length) toast.error(text, action ? { action } : undefined);
  else toast.success(text, action ? { action } : undefined);

  return { ok, failed };
}
