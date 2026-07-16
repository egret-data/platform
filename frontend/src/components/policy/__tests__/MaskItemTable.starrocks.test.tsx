import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MaskItemTable } from "@/components/policy/MaskItemTable";

// MaskItemTable's Groups/Users cells render TagCombobox, which calls
// useQuery — needs a QueryClientProvider ancestor even though its query is
// `enabled: open` (closed by default) and never fires here.
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

test("renders the passed mask types, not the Trino defaults", () => {
  wrap(
    <MaskItemTable
      items={[{ users: [], groups: [], maskType: "MASK_HASH", valueExpr: undefined }]}
      onChange={() => {}}
      maskTypes={[
        { name: "MASK_HASH", label: "Hash" },
        { name: "MASK_DATE_SHOW_YEAR", label: "Date: year only" },
      ]}
    />,
  );
  // Trino-only option must be absent when StarRocks mask types are supplied.
  expect(screen.queryByText("Show last 4")).toBeNull();
});
