/**
 * JSON READ WITHOUT ROUNDING A NUMBER (W7d Task 5).
 *
 * W9c keeps every digit of a KV number (the user's ruling, 2026-10-07), and
 * `GET …/kv/{key}` sends an int or a decimal as a JSON number with all of
 * them (`KvNumber::to_json`, an `arbitrary_precision` crate). `JSON.parse`
 * turns `12345678901234567890` into `12345678901234567000` and `1.50` into
 * `1.5`, so a value read that way and saved back would change the key. This
 * reader keeps each number as its source text ({@link JsonNumber}), and
 * {@link stringifyLossless} writes it back as it came.
 */

/** A JSON number as the server wrote it. */
export class JsonNumber {
  constructor(readonly text: string) {}
  toString(): string {
    return this.text;
  }
}

export type LosslessJson = null | boolean | string | JsonNumber | LosslessJson[] | { [key: string]: LosslessJson };

/** Parse `text` as JSON, every number a {@link JsonNumber}. Throws `SyntaxError` on malformed JSON. */
export function parseLossless(text: string): LosslessJson {
  let i = 0;
  const fail = (what: string): never => {
    throw new SyntaxError(`${what} at position ${i}`);
  };
  const ws = () => {
    while (i < text.length && " \t\n\r".includes(text[i])) i++;
  };
  const value = (): LosslessJson => {
    ws();
    const c = text[i];
    if (c === "{") {
      i++;
      const out: { [key: string]: LosslessJson } = {};
      ws();
      if (text[i] === "}") {
        i++;
        return out;
      }
      for (;;) {
        ws();
        if (text[i] !== '"') fail("expected a field name");
        const k = string();
        ws();
        if (text[i] !== ":") fail("expected ':'");
        i++;
        // Defined, not assigned: a field named `__proto__` is data, not the prototype.
        Object.defineProperty(out, k, { value: value(), enumerable: true, writable: true, configurable: true });
        ws();
        if (text[i] === ",") {
          i++;
          continue;
        }
        if (text[i] === "}") {
          i++;
          return out;
        }
        fail("expected ',' or '}'");
      }
    }
    if (c === "[") {
      i++;
      const out: LosslessJson[] = [];
      ws();
      if (text[i] === "]") {
        i++;
        return out;
      }
      for (;;) {
        out.push(value());
        ws();
        if (text[i] === ",") {
          i++;
          continue;
        }
        if (text[i] === "]") {
          i++;
          return out;
        }
        fail("expected ',' or ']'");
      }
    }
    if (c === '"') return string();
    for (const [word, v] of [["true", true], ["false", false], ["null", null]] as const) {
      if (text.startsWith(word, i)) {
        i += word.length;
        return v;
      }
    }
    const m = /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/.exec(text.slice(i));
    if (!m) fail("unexpected character");
    i += m![0].length;
    return new JsonNumber(m![0]);
  };
  const string = (): string => {
    // From the opening quote to its closing one, escapes included; `JSON.parse`
    // decodes the escapes exactly as it would anywhere else.
    const start = i;
    i++;
    while (i < text.length && text[i] !== '"') i += text[i] === "\\" ? 2 : 1;
    if (i >= text.length) fail("unterminated string");
    i++;
    return JSON.parse(text.slice(start, i)) as string;
  };
  const out = value();
  ws();
  if (i !== text.length) fail("unexpected text after the value");
  return out;
}

/** `JSON.stringify` of a {@link LosslessJson}, every number written as it was read. */
export function stringifyLossless(v: LosslessJson, indent = 0): string {
  const pad = (depth: number) => (indent ? `\n${" ".repeat(indent * depth)}` : "");
  const walk = (x: LosslessJson, depth: number): string => {
    if (x instanceof JsonNumber) return x.text;
    if (Array.isArray(x)) {
      if (x.length === 0) return "[]";
      return `[${x.map((e) => pad(depth + 1) + walk(e, depth + 1)).join(",")}${pad(depth)}]`;
    }
    if (x !== null && typeof x === "object") {
      const entries = Object.entries(x);
      if (entries.length === 0) return "{}";
      const sep = indent ? ": " : ":";
      return `{${entries.map(([k, e]) => pad(depth + 1) + JSON.stringify(k) + sep + walk(e, depth + 1)).join(",")}${pad(depth)}}`;
    }
    return JSON.stringify(x);
  };
  return walk(v, 0);
}
