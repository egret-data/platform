package io.kestra.core.runners;

import java.util.ArrayList;
import java.util.Collection;
import java.util.Comparator;
import java.util.List;

/**
 * The secret values this run resolved, and the one function that hides them (W5b fix round 1, I1).
 *
 * <p>egret passes every value {@code secret()} handed out in {@code task.json}'s {@code mask} array --
 * the file already holds them in the task's properties, so this adds no exposure. Before any byte
 * reaches the container's stdout or stderr ({@link MaskingOutputStream}, {@link MaskingLayout}), each
 * occurrence of each value is replaced by {@link #MASK} -- every byte but the outputs marker
 * (see {@link EgretPluginHost#outputsLine}). Without this the host's stdout -- which is
 * what {@code kubectl logs}, a node's log shipper and Docker's json-file driver keep -- held every
 * secret a plugin logged; egret masked only its own copy.
 *
 * <p>The same algorithm as egret's {@code logs::mask}: one left-to-right scan taking the
 * <b>longest</b> value that matches at each position, so a value that is a substring of another
 * cannot leave the longer one's tail readable, and a value made of {@code *} cannot re-match the mask
 * it just produced. Empty values mask nothing.
 */
final class Masker {
    static final String MASK = "******";

    private static volatile List<String> values = List.of();

    private Masker() {
    }

    /** Replace the values to mask: non-empty, distinct, longest first. */
    static void set(Collection<String> newValues) {
        List<String> out = new ArrayList<>();
        for (String v : newValues) {
            if (v != null && !v.isEmpty() && !out.contains(v)) {
                out.add(v);
            }
        }
        out.sort(Comparator.comparingInt(String::length).reversed().thenComparing(Comparator.naturalOrder()));
        values = List.copyOf(out);
    }

    static boolean active() {
        return !values.isEmpty();
    }

    /** {@code text} with every value replaced by {@link #MASK}. */
    static String mask(String text) {
        List<String> vs = values;
        if (text == null || vs.isEmpty()) {
            return text;
        }
        boolean any = false;
        for (String v : vs) {
            if (text.contains(v)) {
                any = true;
                break;
            }
        }
        if (!any) {
            return text;
        }
        StringBuilder out = new StringBuilder(text.length());
        int i = 0;
        outer:
        while (i < text.length()) {
            for (String v : vs) {
                if (text.startsWith(v, i)) {
                    out.append(MASK);
                    i += v.length();
                    continue outer;
                }
            }
            out.append(text.charAt(i));
            i++;
        }
        return out.toString();
    }
}
