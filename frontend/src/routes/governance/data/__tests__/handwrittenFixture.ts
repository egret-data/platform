import type { HandWrittenPolicy, ObjectHandWritten } from "@/api/generated/model";

/** Route A's answer for a Data placement test: one access, one masking and one row-filter row. */
export function handwrittenAnswer(catalogId: string): { status: number; body: ObjectHandWritten } {
  const row = (id: number, name: string, policyType: string): HandWrittenPolicy => ({
    id,
    name,
    service: "lake-tr",
    inTagService: false,
    policyType,
    match: "applies",
    throughPattern: false,
    viaTag: null,
    resourceSummary: "catalog=ice; schema=sales; table=orders",
    enabled: true,
    denyAllElse: false,
    unmodelled: [],
    items: [],
    parseError: null,
  });
  return {
    status: 200,
    body: {
      catalog: { id: catalogId, name: "ice" },
      target: {},
      state: "listed",
      reason: null,
      profiles: [{ serviceId: 2, name: "lake-tr", type: "trino", tagService: null, engines: [] }],
      policies: [row(1, "hw-access", "access"), row(2, "hw-mask", "masking"), row(3, "hw-filter", "rowfilter")],
      seeded: [],
    },
  };
}
