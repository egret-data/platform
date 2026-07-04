import { isValidQuantity } from "./QuantityInput";

test("accepts k8s quantities + trino sizes", () => {
  ["4Gi", "500m", "2GB", "1000", "1.5", "250Mi", ""].forEach((v) =>
    expect(isValidQuantity(v)).toBe(true),
  );
});

test("rejects garbage", () => {
  ["4 gigs", "abc", "4G B", "-1x"].forEach((v) =>
    expect(isValidQuantity(v)).toBe(false),
  );
});
