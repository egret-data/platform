package io.kestra.core.runners;

import ch.qos.logback.classic.PatternLayout;
import ch.qos.logback.classic.spi.ILoggingEvent;

/**
 * logback's {@code PatternLayout}, with the whole formatted event -- {@code %msg} and the stack trace
 * logback appends -- passed through {@link Masker#mask} (W5b fix round 1, I1).
 *
 * <p>The host's stdout is also masked as a stream ({@link MaskingOutputStream}); this layer keeps a log
 * event masked whatever stream logback writes it to.
 */
public final class MaskingLayout extends PatternLayout {
    @Override
    public String doLayout(ILoggingEvent event) {
        return Masker.mask(super.doLayout(event));
    }
}
