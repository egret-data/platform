import type { ParamOperator, ParamType } from "@/api/dashboards";

/** The operators each parameter type offers, the first being the type's default. */
export const OPERATORS: Record<ParamType, ParamOperator[]> = {
  text: ["is", "is-not", "contains", "does-not-contain", "starts-with", "ends-with"],
  number: ["=", "!=", "between", ">=", "<="],
  date: ["range", "relative", "single", "month-year", "quarter-year", "all-options"],
  id: ["is"],
  category: ["is"],
  boolean: ["is"],
  location: ["is", "is-not"],
  temporal_unit: ["is"],
};
