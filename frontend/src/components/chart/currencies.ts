/**
 * THE NAMED CURRENCIES THE COLUMN-FORMAT PANEL OFFERS — Metabase's list, vendored.
 *
 * The `Currency` control used to be a free-text `<Input>` pre-filled `USD`, so setting a column to
 * Vietnamese dong meant knowing that its ISO 4217 code is `VND`, typing it without a typo, and
 * getting no feedback either way: `formatNumber` hands the stored string to `Intl.NumberFormat` as
 * `opts.currency`, which throws a RangeError on an unknown code — inside the option build, which is
 * how this file's neighbour `safeDecimals` records that a card gets replaced by
 * `ChartErrorBoundary` rather than drawing a wrong number. A picker cannot produce that state.
 *
 * WHERE THE LIST COMES FROM, AND THE TWO ALTERNATIVES THAT WERE MEASURED AND REJECTED.
 *
 * It is generated from `metabase/src/metabase/util/currency.cljc` — the single hand-maintained data
 * file behind Metabase's own picker, read on the frontend through `cljs/metabase.util.currency` and
 * mapped to `{name, value: code}` by `utils/formatting/currency.ts`'s `getCurrencyOptions`. 122
 * entries, in that file's order (USD, CAD, EUR first, then alphabetical by code), which is the order
 * Metabase's select shows them in.
 *
 *   1. `Intl.supportedValuesOf("currency")` — REJECTED, because the list it returns is a property of
 *      the ICU build, not of the language. Measured: 162 codes under this machine's Node 24, and the
 *      count moves with the engine and its ICU version. That makes the set of currencies egret
 *      offers depend on which browser the reader happens to be using — a chart authored in one could
 *      name a currency the picker in another cannot show — and makes any test over the option list
 *      non-deterministic. It also omits BTC and ETH, which Metabase offers and which
 *      `Intl.NumberFormat` formats perfectly well as `BTC 1.00`.
 *   2. `Intl.DisplayNames(["en"], { type: "currency" })` for the names — REJECTED for the same
 *      CLDR-version reason and one of its own: measured against this list, 16 of the 122 names come
 *      back different ("British Pound" for Metabase's "British Pound Sterling", "Czech Koruna" for
 *      "Czech Republic Koruna"), and BTC and ETH have no CLDR name at all, so `dn.of("BTC")` returns
 *      the code back. Deriving the names would mean a picker that is nearly-but-not-quite the
 *      reference's, which is the state this parity programme exists to leave.
 *
 * Vendoring costs a refresh when Metabase adds a currency. That is the trade taken knowingly: a
 * currency list changes on the order of once a year, and the alternative is a control whose contents
 * differ between two readers of the same dashboard.
 *
 * WHAT IS STORED IS THE CODE, NOT THE NAME, and that is not negotiable — `format.ts`'s
 * `formatNumber` passes `fmt.currency` straight to `Intl.NumberFormat` as `opts.currency`, and every
 * chart already saved holds a code there. The name exists only to be searched and read.
 *
 * Regenerated with (from the metabase checkout):
 *   node -e 'const s=require("fs").readFileSync("src/metabase/util/currency.cljc","utf8");
 *     const re=/\[:([A-Z]{3,4})\s+\(make-currency\s+"((?:[^"\\]|\\.)*)"\s+"((?:[^"\\]|\\.)*)"/g;
 *     let m; while((m=re.exec(s))) console.log(m[1], m[3]);'
 */
export interface CurrencyOption {
  /** ISO 4217 code (plus BTC/ETH, which Metabase carries and Intl accepts). This is what is stored. */
  code: string;
  /** Metabase's display name, verbatim. Shown in the picker and matched against the search text. */
  name: string;
}

export const CURRENCIES: readonly CurrencyOption[] = [
  { code: "USD", name: "US Dollar" },
  { code: "CAD", name: "Canadian Dollar" },
  { code: "EUR", name: "Euro" },
  { code: "AED", name: "United Arab Emirates Dirham" },
  { code: "AFN", name: "Afghan Afghani" },
  { code: "ALL", name: "Albanian Lek" },
  { code: "AMD", name: "Armenian Dram" },
  { code: "ARS", name: "Argentine Peso" },
  { code: "AUD", name: "Australian Dollar" },
  { code: "AZN", name: "Azerbaijani Manat" },
  { code: "BAM", name: "Bosnia-Herzegovina Convertible Mark" },
  { code: "BDT", name: "Bangladeshi Taka" },
  { code: "BGN", name: "Bulgarian Lev" },
  { code: "BHD", name: "Bahraini Dinar" },
  { code: "BIF", name: "Burundian Franc" },
  { code: "BND", name: "Brunei Dollar" },
  { code: "BOB", name: "Bolivian Boliviano" },
  { code: "BRL", name: "Brazilian Real" },
  { code: "BTC", name: "Bitcoin" },
  { code: "BWP", name: "Botswanan Pula" },
  { code: "BYR", name: "Belarusian Ruble" },
  { code: "BZD", name: "Belize Dollar" },
  { code: "CDF", name: "Congolese Franc" },
  { code: "CHF", name: "Swiss Franc" },
  { code: "CLP", name: "Chilean Peso" },
  { code: "CNY", name: "Chinese Yuan" },
  { code: "COP", name: "Colombian Peso" },
  { code: "CRC", name: "Costa Rican Colón" },
  { code: "CVE", name: "Cape Verdean Escudo" },
  { code: "CZK", name: "Czech Republic Koruna" },
  { code: "DJF", name: "Djiboutian Franc" },
  { code: "DKK", name: "Danish Krone" },
  { code: "DOP", name: "Dominican Peso" },
  { code: "DZD", name: "Algerian Dinar" },
  { code: "EGP", name: "Egyptian Pound" },
  { code: "ERN", name: "Eritrean Nakfa" },
  { code: "ETB", name: "Ethiopian Birr" },
  { code: "ETH", name: "Ethereum" },
  { code: "GBP", name: "British Pound Sterling" },
  { code: "GEL", name: "Georgian Lari" },
  { code: "GHS", name: "Ghanaian Cedi" },
  { code: "GNF", name: "Guinean Franc" },
  { code: "GTQ", name: "Guatemalan Quetzal" },
  { code: "HKD", name: "Hong Kong Dollar" },
  { code: "HNL", name: "Honduran Lempira" },
  { code: "HRK", name: "Croatian Kuna" },
  { code: "HUF", name: "Hungarian Forint" },
  { code: "IDR", name: "Indonesian Rupiah" },
  { code: "ILS", name: "Israeli New Shekel" },
  { code: "INR", name: "Indian Rupee" },
  { code: "IQD", name: "Iraqi Dinar" },
  { code: "IRR", name: "Iranian Rial" },
  { code: "ISK", name: "Icelandic Króna" },
  { code: "JMD", name: "Jamaican Dollar" },
  { code: "JOD", name: "Jordanian Dinar" },
  { code: "JPY", name: "Japanese Yen" },
  { code: "KES", name: "Kenyan Shilling" },
  { code: "KGS", name: "Kyrgyz Som" },
  { code: "KHR", name: "Cambodian Riel" },
  { code: "KMF", name: "Comorian Franc" },
  { code: "KRW", name: "South Korean Won" },
  { code: "KWD", name: "Kuwaiti Dinar" },
  { code: "KZT", name: "Kazakhstani Tenge" },
  { code: "LBP", name: "Lebanese Pound" },
  { code: "LKR", name: "Sri Lankan Rupee" },
  { code: "LTL", name: "Lithuanian Litas" },
  { code: "LVL", name: "Latvian Lats" },
  { code: "LYD", name: "Libyan Dinar" },
  { code: "MAD", name: "Moroccan Dirham" },
  { code: "MDL", name: "Moldovan Leu" },
  { code: "MGA", name: "Malagasy Ariary" },
  { code: "MKD", name: "Macedonian Denar" },
  { code: "MMK", name: "Myanma Kyat" },
  { code: "MOP", name: "Macanese Pataca" },
  { code: "MRU", name: "Mauritania Ouguiya" },
  { code: "MUR", name: "Mauritian Rupee" },
  { code: "MXN", name: "Mexican Peso" },
  { code: "MYR", name: "Malaysian Ringgit" },
  { code: "MZN", name: "Mozambican Metical" },
  { code: "NAD", name: "Namibian Dollar" },
  { code: "NGN", name: "Nigerian Naira" },
  { code: "NIO", name: "Nicaraguan Córdoba" },
  { code: "NOK", name: "Norwegian Krone" },
  { code: "NPR", name: "Nepalese Rupee" },
  { code: "NZD", name: "New Zealand Dollar" },
  { code: "OMR", name: "Omani Rial" },
  { code: "PAB", name: "Panamanian Balboa" },
  { code: "PEN", name: "Peruvian Nuevo Sol" },
  { code: "PGK", name: "Papua New Guinean Kina" },
  { code: "PHP", name: "Philippine Peso" },
  { code: "PKR", name: "Pakistani Rupee" },
  { code: "PLN", name: "Polish Zloty" },
  { code: "PYG", name: "Paraguayan Guarani" },
  { code: "QAR", name: "Qatari Rial" },
  { code: "RON", name: "Romanian Leu" },
  { code: "RSD", name: "Serbian Dinar" },
  { code: "RUB", name: "Russian Ruble" },
  { code: "RWF", name: "Rwandan Franc" },
  { code: "SAR", name: "Saudi Riyal" },
  { code: "SDG", name: "Sudanese Pound" },
  { code: "SEK", name: "Swedish Krona" },
  { code: "SGD", name: "Singapore Dollar" },
  { code: "SOS", name: "Somali Shilling" },
  { code: "SYP", name: "Syrian Pound" },
  { code: "THB", name: "Thai Baht" },
  { code: "TND", name: "Tunisian Dinar" },
  { code: "TOP", name: "Tongan Paʻanga" },
  { code: "TRY", name: "Turkish Lira" },
  { code: "TTD", name: "Trinidad and Tobago Dollar" },
  { code: "TWD", name: "New Taiwan Dollar" },
  { code: "TZS", name: "Tanzanian Shilling" },
  { code: "UAH", name: "Ukrainian Hryvnia" },
  { code: "UGX", name: "Ugandan Shilling" },
  { code: "UYU", name: "Uruguayan Peso" },
  { code: "UZS", name: "Uzbekistan Som" },
  { code: "VEF", name: "Venezuelan Bolívar" },
  { code: "VND", name: "Vietnamese Dong" },
  { code: "XAF", name: "CFA Franc BEAC" },
  { code: "XOF", name: "CFA Franc BCEAO" },
  { code: "YER", name: "Yemeni Rial" },
  { code: "ZAR", name: "South African Rand" },
  { code: "ZMK", name: "Zambian Kwacha" },
];
