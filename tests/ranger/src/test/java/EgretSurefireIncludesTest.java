import static org.junit.Assert.assertEquals;

import java.io.File;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.junit.Test;

/**
 * Every test class on disk is named in surefire's {@code <includes>} — because that list is an
 * ALLOW-LIST, and a class missing from it compiles, never runs, and leaves the build green.
 *
 * <p><b>SP4d-2b Task 7 fix round 2, and it is here because it already happened.</b> Fix round 1
 * added {@code EgretTagAttachmentMatchTypeTest}, the probe that measures the matcher's wildcard and
 * case behaviour — the measurement the whole round turned on. It compiled, surefire never ran it,
 * and {@code mvn test} reported {@code BUILD SUCCESS} with {@code Tests run: 14}. It was caught only
 * by comparing the per-class lines in the log against the total, which nobody does twice.
 *
 * <p>The {@code <includes>} list exists on purpose: surefire's defaults are {@code Test*},
 * {@code *Test}, {@code *Tests}, {@code *TestCase}, none of which matches {@code EgretCaseRunner} —
 * the differential's own case-file runner, which must keep a name that says what it is. So the list
 * cannot be deleted, and this test is the cheap way to make the next omission loud instead of green.
 *
 * <p>It checks BOTH directions. A class on disk and not in the list is the silent-green failure
 * above; a name in the list with no class on disk is a rename that will start silently running
 * nothing the day the stale entry is the only thing holding a pattern open.
 */
public class EgretSurefireIncludesTest {

    @Test
    public void everyTestClassOnDiskIsNamedInTheSurefireIncludeList() throws Exception {
        File dir = new File("src/test/java");
        List<String> onDisk = new ArrayList<>();
        File[] files = dir.listFiles();
        if (files != null) {
            for (File f : files) {
                if (f.getName().endsWith(".java")) {
                    onDisk.add(f.getName());
                }
            }
        }
        Collections.sort(onDisk);

        String pom = new String(Files.readAllBytes(Paths.get("pom.xml")), "UTF-8");
        List<String> included = new ArrayList<>();
        Matcher m = Pattern.compile("<include>([^<]+)</include>").matcher(pom);
        while (m.find()) {
            included.add(m.group(1).trim());
        }
        Collections.sort(included);

        assertEquals(
                "surefire's <includes> is an allow-list: a .java in src/test/java that is not named "
                        + "there COMPILES AND NEVER RUNS, and the build stays green. Add it to "
                        + "tests/ranger/pom.xml. (A name here with no file is a stale entry from a "
                        + "rename.) on disk vs included",
                onDisk,
                included);
    }
}
