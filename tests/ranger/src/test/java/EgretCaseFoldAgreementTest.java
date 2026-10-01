import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.junit.Test;

/**
 * SP4d-2b Task 7 fix round 2: WHERE the two case folds disagree, measured instead of assumed.
 *
 * <p><b>The two folds.</b> {@code resolved_tags_for_resource_via} compares
 * {@code lower(a.schema_name) = lower($2)} — a FULL-STRING Unicode lowercase on both sides.
 * {@code RangerDefaultResourceMatcher} with {@code ignoreCase: true} compares with
 * {@code StringUtils.equalsIgnoreCase}, which is {@code String.regionMatches(true, …)}: a
 * PER-CHARACTER {@code Character.toUpperCase}/{@code toLowerCase} pair, after a length check. Two
 * different algorithms, so "the query reproduces the matcher" is a claim about where they agree.
 *
 * <p><b>Why this test exists.</b> Fix round 1 could not prove the equivalence outside ASCII, so it
 * declined ANY non-ASCII stored value — and this is a Vietnamese-language deployment, where a
 * diacritic in a table or column name is ordinary. That would have been a visible, recurring
 * over-refusal resting on an un-measured premise. The premise is narrower than the gate was: the
 * disagreement set is **25 code points in the whole of Unicode**, and no Vietnamese letter is in it.
 *
 * <p><b>⚠ What is measured here and what is a STAND-IN.</b> The matcher's side is measured exactly:
 * {@code String.equalsIgnoreCase} is the method {@code StringUtils.equalsIgnoreCase} delegates to.
 * The query's side is {@code String.toLowerCase(Locale.ROOT)}, which is **a stand-in for Postgres
 * {@code lower()}** — both implement Unicode's full {@code Lowercase_Mapping} (including the
 * length-changing {@code SpecialCasing} entries and the {@code Final_Sigma} context rule), and
 * {@code Locale.ROOT} excludes the locale-tailored Turkish/Lithuanian mappings that a
 * {@code tr_TR} database collation would apply.
 *
 * <p><b>This harness runs no Postgres</b> — it has no JDBC dependency — so no row below is
 * asserted against one. ⚠ But a Postgres IS reachable from this worktree, and fix round 3
 * measured the whole set against it (PostgreSQL 16.15, {@code en_US.utf8}, read-only
 * {@code SELECT lower(…)}, no template touched): <b>51 of the 53 pairs the set is derived from
 * still disagree there, and 2 do not</b> — see the ⚠ block on {@link #DOCUMENTED} for which, and
 * for the two claims that are stand-in-only as a result. The earlier wording here said "No Postgres
 * runs in this harness" and left it there, which reads as "not verifiable" when it was merely "not
 * verified here".
 *
 * <p>What would still falsify the set: a collation that tailors case ({@code tr_TR} would move
 * {@code I}/{@code i} INTO it), or an ICU/glibc difference on one of the 25. The direction of the
 * residual is named on {@code egret_perm_compile::routes::FOLD_DIVERGENT} and on
 * {@code stored_coordinate_is_fold_divergent}.
 *
 * <p>This class measures the JDK's Unicode tables, not Ranger — the jar is irrelevant to it. It
 * lives here because this is where the Java-side oracles live, and because the claim it settles is
 * one a Rust test cannot make.
 */
public class EgretCaseFoldAgreementTest {

    /** Where the one source of truth lives — parsed, never duplicated. See {@link #gatedSet()}. */
    private static final String RUST_GATE = "../../crates/egret-perm-compile/src/routes.rs";

    /**
     * The gated set, read out of the RUST constant {@code FOLD_DIVERGENT} itself.
     *
     * <p><b>⚠ Fix round 3, MINOR 1.</b> This class used to hold its own {@code int[] DIVERGENT},
     * and the sweep asserted equality against THAT — so the array the gate actually uses was
     * compared to nothing. The claim in the Task 7 report ("asserted verbatim … and it fails on
     * either direction of drift") was false by code path: deleting {@code U+1C85} from the Rust
     * constant (a fail-open) or mistyping {@code U+03F1} (an over-refusal) failed no test, because
     * {@code [char; 25]} pins only the LENGTH and the Rust tests name 7 of the 25.
     *
     * <p>So there is one copy now and this test reads it. A set is a set in either direction: a code
     * point the sweep finds and Rust does not have is a fail-open, one Rust has and the sweep does
     * not find is an over-refusal, and {@link #theDisagreementSetIsExactlyTheOneTheGateEncodes()}
     * fails on both. The parse is deliberately strict — it asserts the constant was found and that
     * the declared length matches the number of entries — because a silently-empty parse would make
     * this test vacuous, which is the failure mode it exists to remove.
     */
    private static List<Integer> gatedSet() throws Exception {
        String rust = new String(Files.readAllBytes(Paths.get(RUST_GATE)), "UTF-8");
        Matcher decl =
                Pattern.compile("const FOLD_DIVERGENT: \\[char; (\\d+)\\] = \\[(.*?)\\];", Pattern.DOTALL)
                        .matcher(rust);
        assertTrue(
                "FOLD_DIVERGENT must be findable in " + RUST_GATE + " — a parse that matches nothing "
                        + "makes this whole test vacuous",
                decl.find());
        int declaredLength = Integer.parseInt(decl.group(1));
        List<Integer> cps = new ArrayList<>();
        Matcher m = Pattern.compile("'\\\\u\\{([0-9A-Fa-f]{1,6})\\}'").matcher(decl.group(2));
        while (m.find()) {
            cps.add(Integer.parseInt(m.group(1), 16));
        }
        assertEquals(
                "every entry of FOLD_DIVERGENT must parse as a `'\\u{…}'` char literal, and the "
                        + "declared array length must match how many were found",
                declaredLength,
                cps.size());
        Collections.sort(cps);
        return cps;
    }

    /**
     * The code points the gate declines, as documentation — the authoritative list is the Rust
     * constant {@link #gatedSet()} reads.
     *
     * <p>Derived, not hand-listed — see {@link #theDisagreementSetIsExactlyTheOneTheGateEncodes()}
     * for the sweep that produces it over every defined code point. Grouped by the mechanism that
     * puts each one here:
     *
     * <ul>
     *   <li><b>asymmetric 1:1 mapping</b> — {@code toLowerCase(toUpperCase(c)) != toLowerCase(c)},
     *       so the per-character fold calls {@code c} equal to a character the full lowercase does
     *       not: the Turkish dotless {@code ı}, the long {@code ſ}, the Greek final sigma and the
     *       Greek/Cyrillic variant letters, the micro sign;
     *   <li><b>length-changing full lowercase</b> — {@code İ} lowercases to {@code i} + combining
     *       dot above, two code points, so the length check in {@code equalsIgnoreCase} and the
     *       string comparison after {@code lower()} see different things;
     *   <li><b>context-dependent full lowercase</b> — {@code Σ} lowercases to {@code ς} at the end
     *       of a word and {@code σ} elsewhere, which no per-character fold can reproduce.
     * </ul>
     *
     * <p><b>⚠ What the real database says, measured — and two of these are stand-in-only.</b> Fix
     * round 3 ran {@code lower()} on a real PostgreSQL 16.15, {@code en_US.utf8}, read-only
     * ({@code SELECT lower(…)} needs no schema and touches no template). Of the 53 single-character
     * pairs this set is derived from, <b>51 still disagree there</b> and 2 do not: both of
     * {@code U+0130}'s. So:
     *
     * <ul>
     *   <li>{@code U+0130} is a <b>measured over-refusal on en_US.utf8</b> — glibc's
     *       {@code lower('İ')} is {@code 'i'}, length 1, not the two-code-point full mapping the
     *       Java stand-in produces. It stays gated because {@code tr_TR} is a collation where it
     *       earns its place, and because the direction is conservative;
     *   <li>criterion 3's <i>justification</i> (the string pair {@code ("ΑΣ","ασ")}) is
     *       <b>stand-in-only</b>: glibc has no {@code Final_Sigma}, so {@code lower('ΑΣ')='ασ'}
     *       there and that pair AGREES. The criterion's <i>conclusion</i> is right on both oracles
     *       anyway — {@code U+03A3} is gated, and on the real database it is load-bearing through
     *       its single-character pair with {@code U+03C2}, which disagrees there. An ICU collation
     *       does implement {@code Final_Sigma}, which is why the criterion stays.
     * </ul>
     *
     * <p>Every other row of {@link #expectedPairs()} behaves on the real database exactly as it does
     * here, Vietnamese included. The measurement is recorded in the Task 7 report; this harness has
     * no JDBC dependency and does not assert it.
     */
    private static final int[] DOCUMENTED = {
        0x00B5, 0x0130, 0x0131, 0x017F, 0x0345, 0x03A3, 0x03C2, 0x03D0, 0x03D1, 0x03D5, 0x03D6,
        0x03F0, 0x03F1, 0x03F5, 0x1C80, 0x1C81, 0x1C82, 0x1C83, 0x1C84, 0x1C85, 0x1C86, 0x1C87,
        0x1C88, 0x1E9B, 0x1FBE
    };

    /**
     * The named table the ruling asked for: one row per family, with the two folds' answers side by
     * side. A row is a DISAGREEMENT when {@code equalsIgnoreCase} and the {@code lower()} stand-in
     * differ — which is a fail-open or a fail-closed in the resolver depending on direction, and
     * either way a wrong verdict.
     *
     * <p>The Vietnamese rows are the point of the whole round: five letter pairs and a whole word in
     * both cases, all AGREEING, so the fold reproduces the matcher for them and
     * {@code stored_coordinate_is_fold_divergent} must not decline them.
     */
    private static Map<String, String> expectedPairs() {
        Map<String, String> t = new LinkedHashMap<>();
        // The Turkish I family — the classic divergence, and both directions of it.
        t.put("I|ı", "DISAGREE eic=true lower=false");
        t.put("İ|i", "DISAGREE eic=true lower=false");
        // ASCII, the control: if this disagreed, the whole fold would be wrong.
        t.put("i|I", "AGREE eic=true lower=true");
        t.put("sales|SALES", "AGREE eic=true lower=true");
        // The other asymmetric 1:1 pairs. `ſ` is the one that proves the REQUEST side has to
        // be checked too: the stored value can be pure ASCII (`s`) and the request the odd one.
        t.put("ſ|s", "DISAGREE eic=true lower=false");
        t.put("µ|μ", "DISAGREE eic=true lower=false");
        // Greek sigma, isolated and in context — the only context-dependent lowercase in ROOT.
        t.put("Σ|ς", "DISAGREE eic=true lower=false");
        t.put("ΑΣ|ασ", "DISAGREE eic=true lower=false");
        // Vietnamese: every one AGREES, which is what narrows the gate.
        t.put("Ạ|ạ", "AGREE eic=true lower=true");
        t.put("Ế|ế", "AGREE eic=true lower=true");
        t.put("Đ|đ", "AGREE eic=true lower=true");
        t.put("Ơ|ơ", "AGREE eic=true lower=true");
        t.put("Ư|ư", "AGREE eic=true lower=true");
        t.put("Tiếng|TIẾNG", "AGREE eic=true lower=true");
        // ⚠ The uppercase here is U+1EDC (Ờ), the capital of U+1EDD (ờ). The first
        // version of this row used U+1EEC (Ử) and the table CAUGHT it: two different words
        // fold to `eic=false lower=false`, which still AGREES but on the other value — a pair
        // of agreeing falses would have read as a passing Vietnamese row while measuring nothing.
        t.put("người|NGƯỜI", "AGREE eic=true lower=true");
        // German sharp s, both shapes.
        t.put("ß|SS", "AGREE eic=false lower=false");
        t.put("ß|ẞ", "AGREE eic=true lower=true");
        // Cyrillic, and a compatibility character whose simple and full mappings coincide.
        t.put("Д|д", "AGREE eic=true lower=true");
        t.put("K|k", "AGREE eic=true lower=true");
        // Negative controls: different letters must be unequal under BOTH folds, or every
        // AGREE above is satisfied by a comparison that says yes to everything.
        t.put("ạ|a", "AGREE eic=false lower=false");
        t.put("sales|orders", "AGREE eic=false lower=false");
        return t;
    }

    @Test
    public void theNamedPairsFoldTheWayTheTableSays() {
        Map<String, String> measured = new LinkedHashMap<>();
        for (String row : expectedPairs().keySet()) {
            String[] parts = row.split("\\|", -1);
            boolean eic = parts[0].equalsIgnoreCase(parts[1]);
            boolean low =
                    parts[0]
                            .toLowerCase(Locale.ROOT)
                            .equals(parts[1].toLowerCase(Locale.ROOT));
            measured.put(
                    row,
                    String.format("%s eic=%s lower=%s", eic == low ? "AGREE" : "DISAGREE", eic, low));
        }
        assertEquals(
                "the two folds must behave exactly as this table says — it is what "
                        + "`FOLD_DIVERGENT`, the gate's divergent-character set, is derived from, and what "
                        + "makes 'the fold reproduces ignoreCase' a claim with a population",
                expectedPairs(),
                measured);
    }

    /**
     * The sweep that makes {@link #DIVERGENT} a DERIVED set rather than a hand-list, over every
     * defined code point in Unicode — not just the BMP, because Deseret, Adlam, Warang Citi and
     * Medefaidrin all have case pairs and a hand-list would have no reason to think of them.
     *
     * <p>Three criteria, each the negation of a step in the proof that the two folds coincide for a
     * string of unflagged characters:
     *
     * <ol>
     *   <li>the full lowercase of the character, in isolation, is not exactly its simple lowercase
     *       (so the per-character fold and {@code lower()} disagree on the character itself, or
     *       {@code lower()} changes the length and the length check in {@code equalsIgnoreCase}
     *       sees a different string);
     *   <li>{@code toLowerCase(toUpperCase(c)) != toLowerCase(c)} — the per-character fold's
     *       uppercase branch can match a character the lowercase branch would not, which is exactly
     *       how {@code ı} is "equal" to {@code I} and {@code ſ} to {@code s};
     *   <li>the full lowercase of the character DEPENDS on its neighbours, which no per-character
     *       fold can see. {@code Σ} is the only one in {@code Locale.ROOT}, and the pair
     *       {@code ("ΑΣ", "ασ")} — measured DISAGREE in the table above — is why criterion 2 alone
     *       is not enough: neither {@code Σ} nor {@code σ} is caught by it.
     * </ol>
     *
     * <p>With no character of either string flagged, {@code equalsIgnoreCase} and
     * {@code lower()}-equality are the same predicate; so gating a value that contains ANY flagged
     * character — on both the stored and the request side — is sufficient, and gating nothing else
     * is what keeps Vietnamese out of the refusal.
     */
    @Test
    public void theDisagreementSetIsExactlyTheOneTheGateEncodes() throws Exception {
        List<Integer> flagged = new ArrayList<>();
        for (int cp = 0; cp <= Character.MAX_CODE_POINT; cp++) {
            // ⚠ `Character.isSurrogate((char) cp)` was the condition here and it TRUNCATES: for
            // every cp above 0xFFFF the cast wraps, so 6 853 defined code points were skipped and
            // the method's name was false by code path (fix round 3, LOW 2). Inert — none of them
            // is cased in a way that diverges — but a sweep that claims "every defined code point"
            // has to visit them.
            if (!Character.isDefined(cp)
                    || (cp >= Character.MIN_SURROGATE && cp <= Character.MAX_SURROGATE)) {
                continue;
            }
            String isolated = new String(Character.toChars(cp));
            String full = isolated.toLowerCase(Locale.ROOT);
            int simple = Character.toLowerCase(cp);
            boolean suspect =
                    full.codePointCount(0, full.length()) != 1 || full.codePointAt(0) != simple;
            if (!suspect) {
                suspect = Character.toLowerCase(Character.toUpperCase(cp)) != simple;
            }
            if (!suspect) {
                // A cased neighbour on either side, which is what `Final_Sigma` keys on.
                for (String ctx :
                        new String[] {
                            "Α" + isolated, isolated + "Α", "Α" + isolated + "Α"
                        }) {
                    String low = ctx.toLowerCase(Locale.ROOT);
                    if (low.length() != ctx.length()) {
                        continue;
                    }
                    int at = ctx.indexOf(isolated);
                    if (!low.substring(at, at + isolated.length()).equals(full)) {
                        suspect = true;
                        break;
                    }
                }
            }
            if (suspect) {
                flagged.add(cp);
            }
        }

        assertEquals(
                "the derived disagreement set must be exactly the one the RUST constant "
                        + "FOLD_DIVERGENT encodes (read from " + RUST_GATE + ", not a copy here) — a "
                        + "code point the sweep finds and Rust lacks is a fail-open, one Rust has and "
                        + "the sweep does not find is an over-refusal; swept " + hex(flagged),
                gatedSet(),
                flagged);
        // The documented list in this file is prose, so it must not be allowed to rot away from
        // the constant it describes either.
        List<Integer> documented = new ArrayList<>();
        for (int cp : DOCUMENTED) {
            documented.add(cp);
        }
        Collections.sort(documented);
        assertEquals(
                "this file's DOCUMENTED list is commentary on the Rust constant and must match it",
                gatedSet(),
                documented);

        // 25 is small enough to enumerate and that is the whole finding: the fix-round-1 gate
        // declined every non-ASCII value on a premise this narrow.
        assertEquals("the set is 25 code points in the whole of Unicode", 25, flagged.size());
    }

    /**
     * The deployment's own alphabet, stated as an assertion rather than left to the sweep's output:
     * no Vietnamese letter may be in the divergent set, because every one of them in it is a
     * catalog whose every {@code explain} answers {@code TagsUnresolved}.
     */
    @Test
    public void noVietnameseLetterIsInTheDivergentSet() throws Exception {
        Set<Integer> divergent = new LinkedHashSet<>(gatedSet());
        // Every Vietnamese letter that is not a plain ASCII one, upper and lower: the seven base
        // letters with their own code points, and the full tone-marked ranges.
        List<Integer> vietnamese = new ArrayList<>();
        for (int cp :
                new int[] {
                    0x0102, 0x0103, 0x00C2, 0x00E2, 0x00CA, 0x00EA, 0x00D4, 0x00F4, 0x01A0, 0x01A1,
                    0x01AF, 0x01B0, 0x0110, 0x0111, 0x00C0, 0x00E0, 0x00C1, 0x00E1, 0x00C3, 0x00E3,
                    0x00C8, 0x00E8, 0x00C9, 0x00E9, 0x00CC, 0x00EC, 0x00CD, 0x00ED, 0x00D2, 0x00F2,
                    0x00D3, 0x00F3, 0x00D5, 0x00F5, 0x00D9, 0x00F9, 0x00DA, 0x00FA, 0x00DD, 0x00FD
                }) {
            vietnamese.add(cp);
        }
        for (int cp = 0x1EA0; cp <= 0x1EF9; cp++) {
            vietnamese.add(cp);
        }
        List<Integer> offenders = new ArrayList<>();
        for (int cp : vietnamese) {
            if (divergent.contains(cp)) {
                offenders.add(cp);
            }
        }
        assertTrue(
                "no Vietnamese letter may be gated — each one that is refuses a whole catalog: "
                        + hex(offenders),
                offenders.isEmpty());
        // And the control, so this is not vacuous: the Turkish dotted I IS gated.
        assertFalse("the control: U+0130 must be in the set", vietnamese.contains(0x0130));
        assertTrue("the control: U+0130 must be gated", divergent.contains(0x0130));
    }

    private static String hex(List<Integer> cps) {
        StringBuilder b = new StringBuilder();
        for (int cp : cps) {
            b.append(String.format("U+%04X ", cp));
        }
        return b.toString();
    }
}
