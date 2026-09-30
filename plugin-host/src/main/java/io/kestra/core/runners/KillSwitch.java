package io.kestra.core.runners;

import java.io.PrintStream;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/**
 * The body of the host's JVM shutdown hook (W5b spec §7.1).
 *
 * <p>On SIGTERM egret has timed the task out. The hook calls the task's {@code kill()} (for JDBC:
 * {@code Statement.cancel()}) and waits for {@code run()} to return, at most {@code waitMs}. It then
 * returns and lets the JVM halt; it never calls {@code System.exit}, which would deadlock inside a hook.
 *
 * <p>{@code kill()} runs on a thread of its own, so a kill that blocks cannot hold the JVM past the
 * grace: on StarRocks through MySQL Connector/J it blocks until the query ends ({@code isClosed()}
 * takes the connection lock the running statement holds; Task 1).
 *
 * <p>W5b Task 6 fix 1, two changes, both about a Postgres cancel that was never sent (1 live run in 11):
 *
 * <ul>
 *   <li><b>The hook joins the killer</b> after the latch opens, bounded by what is left of
 *       {@code waitMs}. {@code run()} can return while {@code kill()} is still sending the cancel -- any
 *       other shutdown hook that closes the connection makes {@code run()} throw -- and a hook that
 *       returned then would let the JVM halt with the cancel unsent.
 *   <li><b>{@code kill()} is invoked again</b> every {@link #RETRY_MS} until {@code run()} returns. A
 *       SIGTERM that lands before the plugin has its running statement (a cold JVM still connecting)
 *       finds nothing to cancel; the next call does. The JDBC plugins' {@code kill()} is idempotent: it
 *       skips a statement or connection that is already closed.
 * </ul>
 */
final class KillSwitch {
    static final String KILLED_LINE = "WARN killed: timeout";

    /** How often {@code kill()} is invoked again while {@code run()} has not returned. */
    static final long RETRY_MS = 200;

    private final Runnable kill;
    private final long waitMs;
    private final CountDownLatch returned = new CountDownLatch(1);

    KillSwitch(Runnable kill, long waitMs) {
        this.kill = kill;
        this.waitMs = waitMs;
    }

    /** Called once {@code run()} has returned or thrown. */
    void runReturned() {
        returned.countDown();
    }

    void onTerm(PrintStream out) {
        if (returned.getCount() == 0) {
            return;
        }
        out.println(KILLED_LINE);
        out.flush();

        Thread killer = new Thread(() -> {
            try {
                do {
                    kill.run();
                } while (!returned.await(RETRY_MS, TimeUnit.MILLISECONDS));
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            } catch (Throwable t) {
                out.println("WARN kill() threw " + t.getClass().getName() + ": " + t.getMessage());
                out.flush();
            }
        }, "egret-kill");
        killer.setDaemon(true);
        long deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(waitMs);
        killer.start();

        boolean didReturn;
        try {
            didReturn = returned.await(waitMs, TimeUnit.MILLISECONDS);
            // The kill in flight finishes (the cancel is sent) before the JVM may halt.
            long left = TimeUnit.NANOSECONDS.toMillis(deadline - System.nanoTime());
            if (left > 0) {
                killer.join(left);
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            didReturn = false;
        }
        if (!didReturn) {
            out.println("WARN killed: run() did not return within " + waitMs + " ms of kill()");
            out.flush();
        } else if (killer.isAlive()) {
            out.println("WARN killed: kill() did not return within " + waitMs + " ms");
            out.flush();
        }
    }
}
