import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  checkDescription,
  checkKey,
  checkTtl,
  encodeValue,
  headerText,
  remainingTtl,
  valueToText,
} from "@/components/workflows/namespaces/kv/kvValue";
import { JsonNumber, parseLossless, stringifyLossless } from "@/components/workflows/namespaces/kv/losslessJson";

/**
 * The KV form's rules (W7d spec §4.4): the body `PUT …/kv/{key}` gets for each
 * type, which W9c's server reads as ONE Ion value and types by what it reads
 * (`parse_api_body`), and the refusals the form makes before sending.
 *
 * DATETIME is entered in the viewer's time zone; this file pins one with an
 * offset, so a conversion that forgot it fails.
 */
const savedTz = process.env.TZ;
beforeAll(() => {
  process.env.TZ = "Asia/Ho_Chi_Minh";
});
afterAll(() => {
  process.env.TZ = savedTz;
});

const ok = (body: string) => ({ ok: true, body });
const refused = (r: ReturnType<typeof encodeValue>) => (r.ok ? null : r.error);

describe("encodeValue: the body each type sends", () => {
  it("STRING is an Ion string literal, escapes included", () => {
    expect(encodeValue("STRING", "hello world")).toEqual(ok('"hello world"'));
    expect(encodeValue("STRING", 'a "quoted"\nline')).toEqual(ok('"a \\"quoted\\"\\nline"'));
    // Bare text would be a symbol, a number or two values; quoted it is always one string.
    expect(encodeValue("STRING", "42")).toEqual(ok('"42"'));
    expect(encodeValue("STRING", "")).toEqual(ok('""'));
  });

  it("STRING refuses text the server would store as a DURATION, and says what to do", () => {
    for (const text of ["PT1H", "P1D", "P1DT2H", "PT0S"]) {
      expect(refused(encodeValue("STRING", text)), text).toMatch(/reads as a duration.*DURATION type/);
    }
    // Not Kestra's duration pattern: plain strings.
    for (const text of ["PT1.5S", "p1d", "-PT1S", "P", "PT", "P1W"]) {
      expect(encodeValue("STRING", text), text).toEqual(ok(JSON.stringify(text)));
    }
  });

  it("NUMBER is an Ion int or decimal, every digit kept", () => {
    expect(encodeValue("NUMBER", "42")).toEqual(ok("42"));
    expect(encodeValue("NUMBER", " -1.50 ")).toEqual(ok("-1.50"));
    expect(encodeValue("NUMBER", "12345678901234567890.123456789")).toEqual(ok("12345678901234567890.123456789"));
  });

  it("NUMBER with an exponent is an Ion decimal (`d`), never a float (`e`), which would round it", () => {
    expect(encodeValue("NUMBER", "6.02e23")).toEqual(ok("6.02d23"));
    expect(encodeValue("NUMBER", "1E5")).toEqual(ok("1d5"));
    expect(encodeValue("NUMBER", "-2.5e-3")).toEqual(ok("-2.5d-3"));
    // A 20-digit mantissa: as a float it would keep about 17 of them.
    expect(encodeValue("NUMBER", "1.2345678901234567890e5")).toEqual(ok("1.2345678901234567890d5"));
    // As a float, an infinity; as a decimal, 401 digits written out.
    expect(encodeValue("NUMBER", "1e400")).toEqual(ok("1d400"));
  });

  it("NUMBER counts digits as the server writes the decimal out: 1,000 at most", () => {
    expect(encodeValue("NUMBER", "1e999")).toEqual(ok("1d999"));
    expect(refused(encodeValue("NUMBER", "1e1000"))).toMatch(/1,000 digits/);
    expect(encodeValue("NUMBER", "-1e-999")).toEqual(ok("-1d-999"));
    expect(refused(encodeValue("NUMBER", "1e-1000"))).toMatch(/1,000 digits/);
    expect(refused(encodeValue("NUMBER", "1e99999999999"))).toMatch(/1,000 digits/);
  });

  it("NUMBER refuses what is not a JSON number, and more than 1,000 digits", () => {
    for (const text of ["", "abc", "1,5", "+1", "007", ".5", "1.", "1_000", "0x10", "NaN", "1 2"]) {
      expect(refused(encodeValue("NUMBER", text)), text).toMatch(/number/i);
    }
    expect(encodeValue("NUMBER", "9".repeat(1000))).toEqual(ok("9".repeat(1000)));
    expect(refused(encodeValue("NUMBER", "9".repeat(1001)))).toMatch(/1,000 digits/);
    expect(refused(encodeValue("NUMBER", `1.${"0".repeat(1000)}`))).toMatch(/1,000 digits/);
  });

  it("BOOLEAN is true or false", () => {
    expect(encodeValue("BOOLEAN", "true")).toEqual(ok("true"));
    expect(encodeValue("BOOLEAN", "false")).toEqual(ok("false"));
    expect(refused(encodeValue("BOOLEAN", ""))).toMatch(/true or false/);
    expect(refused(encodeValue("BOOLEAN", "yes"))).toMatch(/true or false/);
  });

  it("DATETIME is the local time typed, sent in UTC", () => {
    // Asia/Ho_Chi_Minh is UTC+7.
    expect(encodeValue("DATETIME", "2026-10-14T09:30")).toEqual(ok("2026-10-14T02:30:00.000Z"));
    expect(encodeValue("DATETIME", "2026-01-01T03:04:05.678")).toEqual(ok("2025-12-31T20:04:05.678Z"));
  });

  it("DATETIME refuses a missing value and a year outside 1-9999 in UTC", () => {
    expect(refused(encodeValue("DATETIME", ""))).toMatch(/date and a time/);
    expect(refused(encodeValue("DATETIME", "not a date"))).toMatch(/date and a time/);
    // 0001-01-01 05:00 at UTC+7 is year 0 in UTC.
    expect(refused(encodeValue("DATETIME", "0001-01-01T05:00"))).toMatch(/years 1 to 9999/);
    // Local year 10000 is still 9999 in UTC on its first night here, and accepted; a day later it is not.
    expect(encodeValue("DATETIME", "+010000-01-01T00:00")).toEqual(ok("9999-12-31T17:00:00.000Z"));
    expect(refused(encodeValue("DATETIME", "+010000-01-02T00:00"))).toMatch(/years 1 to 9999/);
  });

  it("DATE is the day as typed", () => {
    expect(encodeValue("DATE", "2026-10-14")).toEqual(ok("2026-10-14"));
    expect(encodeValue("DATE", "0001-01-01")).toEqual(ok("0001-01-01"));
    for (const text of ["", "2026-02-30", "0000-01-01", "2026-1-1", "14/10/2026"]) {
      expect(refused(encodeValue("DATE", text)), text).toMatch(/date/);
    }
  });

  it("DURATION is a quoted Kestra duration", () => {
    expect(encodeValue("DURATION", "PT1H30M")).toEqual(ok('"PT1H30M"'));
    expect(encodeValue("DURATION", "P1D")).toEqual(ok('"P1D"'));
    expect(encodeValue("DURATION", "P2DT3H4M5S")).toEqual(ok('"P2DT3H4M5S"'));
  });

  it("DURATION refuses what the server would store as a STRING", () => {
    for (const text of ["", "1h", "P", "PT", "P1DT", "PT1.5S", "p1d", "-PT1S", "P1W", "P1M", "PTH", `PT${"9".repeat(20)}S`]) {
      expect(refused(encodeValue("DURATION", text)), text).toMatch(/duration/i);
    }
  });

  it("JSON is an object or an array, its numbers sent as NUMBER sends them", () => {
    expect(encodeValue("JSON", '{"a": 12345678901234567890}')).toEqual(ok('{"a":12345678901234567890}'));
    expect(encodeValue("JSON", " [1, 2.50] ")).toEqual(ok("[1,2.50]"));
    expect(encodeValue("JSON", '{"x": [6.02e23, "6.02e23"]}')).toEqual(ok('{"x":[6.02d23,"6.02e23"]}'));
    expect(encodeValue("JSON", "{}")).toEqual(ok("{}"));
    expect(refused(encodeValue("JSON", "[1e1000]"))).toMatch(/1,000 digits/);
  });

  it("JSON refuses malformed text and a scalar, and names the type to use", () => {
    expect(refused(encodeValue("JSON", "{a: 1}"))).toMatch(/not valid JSON/);
    expect(refused(encodeValue("JSON", ""))).toMatch(/not valid JSON/);
    expect(refused(encodeValue("JSON", "42"))).toMatch(/object.*array/);
    expect(refused(encodeValue("JSON", '"text"'))).toMatch(/object.*array/);
    expect(refused(encodeValue("JSON", "null"))).toMatch(/object.*array/);
  });
});

describe("valueToText: a stored value back into the form", () => {
  it("keeps a number's digits and a JSON value's numbers", () => {
    expect(valueToText("NUMBER", new JsonNumber("12345678901234567890.10"))).toBe("12345678901234567890.10");
    const json = parseLossless('{"big": 12345678901234567890, "list": [1.50, true, null, "x"]}');
    expect(valueToText("JSON", json)).toBe(stringifyLossless(json, 2));
    expect(valueToText("JSON", json)).toContain("12345678901234567890");
    expect(valueToText("JSON", json)).toContain("1.50");
  });

  it("shows a stored DATETIME in local time, to the millisecond", () => {
    expect(valueToText("DATETIME", "2026-10-14T02:30:00Z")).toBe("2026-10-14T09:30");
    expect(valueToText("DATETIME", "2025-12-31T20:04:05.678Z")).toBe("2026-01-01T03:04:05.678");
    expect(valueToText("DATETIME", "2025-12-31T20:04:05Z")).toBe("2026-01-01T03:04:05");
  });

  it("keeps the other types' text", () => {
    expect(valueToText("STRING", "hi")).toBe("hi");
    expect(valueToText("STRING", null)).toBe("");
    expect(valueToText("BOOLEAN", false)).toBe("false");
    expect(valueToText("DATE", "2026-10-14")).toBe("2026-10-14");
    expect(valueToText("DURATION", "PT24H")).toBe("PT24H");
  });
});

describe("parseLossless", () => {
  it("reads every JSON form, numbers as their text", () => {
    const v = parseLossless(' {"a": [-0.10, 1e3, "s\\"\\u00e9", false, null], "__proto__": {}} ');
    expect(v).toEqual({ a: [new JsonNumber("-0.10"), new JsonNumber("1e3"), 's"é', false, null], ["__proto__"]: {} });
    expect(Object.getPrototypeOf(v)).toBe(Object.prototype);
    expect(stringifyLossless(v)).toBe('{"a":[-0.10,1e3,"s\\"é",false,null],"__proto__":{}}');
  });

  it("refuses malformed JSON", () => {
    for (const text of ["", "{", "[1,]", "{a:1}", "01", "1 2", '"open']) {
      expect(() => parseLossless(text), text).toThrow(SyntaxError);
    }
  });
});

describe("checkKey: W9c's key rule", () => {
  it("accepts a key of the rule up to 350 bytes", () => {
    expect(checkKey("a")).toBeNull();
    expect(checkKey("my_key-1.v2")).toBeNull();
    expect(checkKey("9lives")).toBeNull();
    expect(checkKey("k".repeat(350))).toBeNull();
  });

  it("refuses the rest, each with its reason", () => {
    expect(checkKey("")).toMatch(/Enter a key/);
    expect(checkKey("_a")).toMatch(/starts with a letter or a digit/);
    expect(checkKey(".a")).toMatch(/starts with a letter or a digit/);
    expect(checkKey("a b")).toMatch(/letters, digits/);
    expect(checkKey("é")).toMatch(/starts with a letter or a digit/);
    expect(checkKey("k".repeat(351))).toMatch(/at most 350/);
  });
});

describe("checkTtl: the `ttl` header's rules", () => {
  // egret's own grammar: `executor::parse_duration` (crates/egret-workflow/src/executor/retry.rs:518)
  // under `parse_ttl` (storage/kv/value.rs:1490). Each number is a Rust f64, so every unit takes a
  // fraction or an exponent (Java's `Duration.parse` would refuse `P1.5D`; egret does not).
  it("accepts what egret's parser accepts", () => {
    for (const text of [
      "PT1H", "P1D", "PT30M", "P1DT12H", "pt5m", "P2dT3h",
      "P1.5D", "PT1.5H", "PT1.5M", "PT1.5S", "P0.5DT1H", "P.5D", "PT5.S", "PT1e3S",
      "P1DT", "+PT1H", " PT1H ",
    ]) {
      expect(checkTtl(text), text).toBeNull();
    }
  });

  it("refuses what egret's parser refuses", () => {
    for (const text of [
      "", "1h", "P", "PT", "P1W", "P1M", "PTH", "PT1S2", "PT1X", "PT30S1M", "P1H",
      "PT-1S", "PTinfS", "PTnanS", "--PT1H", "PT1e20S", "P1,5D",
    ]) {
      expect(checkTtl(text), text).toMatch(/Enter a duration/);
    }
  });

  it("refuses a negative one, zero and over 1,000 years", () => {
    expect(checkTtl("-PT1H")).toMatch(/cannot be negative/);
    expect(checkTtl("PT0S")).toMatch(/expires the key at once/);
    expect(checkTtl("-PT0S")).toMatch(/expires the key at once/);
    expect(checkTtl("P365250D")).toBeNull();
    expect(checkTtl("P365250DT0.5S")).toMatch(/1,000 years/);
    expect(checkTtl("P365251D")).toMatch(/1,000 years/);
  });

  it("the time left before an expiry, as a TTL", () => {
    const now = Date.parse("2026-10-14T00:00:00Z");
    expect(remainingTtl("2026-10-14T01:00:00Z", now)).toBe("PT3600S");
    expect(remainingTtl("2026-10-14T00:00:00.200Z", now)).toBe("PT1S");
    expect(remainingTtl("2026-10-13T00:00:00Z", now)).toBeNull();
  });
});

describe("the description header", () => {
  it("is one line of text", () => {
    expect(checkDescription("Shared token for the ETL")).toBeNull();
    expect(checkDescription("Mã dùng chung")).toBeNull();
    expect(checkDescription("two\nlines")).toMatch(/one line/);
    expect(checkDescription("bell\u0007")).toMatch(/one line/);
  });

  it("is sent as its UTF-8 bytes, which the server reads as UTF-8", () => {
    const sent = headerText("Mã dùng");
    // Every character a byte (what `fetch` puts on the wire), and those bytes the UTF-8 text.
    expect([...sent].every((c) => c.charCodeAt(0) < 256)).toBe(true);
    const bytes = Uint8Array.from([...sent].map((c) => c.charCodeAt(0)));
    expect(new TextDecoder().decode(bytes)).toBe("Mã dùng");
    expect(headerText("plain")).toBe("plain");
  });
});
