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

/** What the Type select calls each parameter type. The keys are stored in the dashboard and in
 *  URLs; an author picks by these labels (KI-067). */
export const PARAM_TYPE_LABELS: Record<ParamType, string> = {
  text: "Text",
  number: "Number",
  date: "Date",
  id: "ID",
  category: "Category",
  boolean: "True / False",
  location: "Location",
  temporal_unit: "Time grouping",
};

/** What the Operator select calls each operator. The keys are stored in the dashboard; an author
 *  picks by these labels (KI-280). */
export const OPERATOR_LABELS: Record<ParamOperator, string> = {
  is: "Is",
  "is-not": "Is not",
  contains: "Contains",
  "does-not-contain": "Does not contain",
  "starts-with": "Starts with",
  "ends-with": "Ends with",
  "=": "Equal to",
  "!=": "Not equal to",
  between: "Between",
  ">=": "Greater than or equal to",
  "<=": "Less than or equal to",
  range: "Date range",
  relative: "Relative date",
  single: "Single date",
  "month-year": "Month and year",
  "quarter-year": "Quarter and year",
  "all-options": "All options",
};
