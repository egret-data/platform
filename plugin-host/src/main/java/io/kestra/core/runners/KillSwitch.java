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
 * <p>{@code kill()} runs on a thread of its own and is never joined: on StarRocks through MySQL
 * Connector/J it blocks until the query ends ({@code isClosed()} takes the connection lock the
 * running statement holds; Task 1), and the JVM must still exit within the grace.
 */
final class KillSwitch {
    static final String KILLED_LINE = "WARN killed: timeout";

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
                kill.run();
            } catch (Throwable t) {
                out.println("WARN kill() threw " + t.getClass().getName() + ": " + t.getMessage());
                out.flush();
            }
        }, "egret-kill");
        killer.setDaemon(true);
        killer.start();

        boolean didReturn;
        try {
            didReturn = returned.await(waitMs, TimeUnit.MILLISECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            didReturn = false;
        }
        if (!didReturn) {
            out.println("WARN killed: run() did not return within " + waitMs + " ms of kill()");
            out.flush();
        }
    }
}
