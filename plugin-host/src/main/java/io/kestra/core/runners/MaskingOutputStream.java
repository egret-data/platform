package io.kestra.core.runners;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;

/**
 * A line-buffered stream that masks every line ({@link Masker#mask}) before it reaches {@code sink}
 * (W5b fix round 1, I1). The host installs one over stdout and one over stderr before it reads
 * {@code task.json}, so everything written to either -- logback's lines and stack traces, the
 * {@code egretFailure} marker, a plugin's {@code System.out} -- passes through it. The one exception
 * is the outputs marker, written through {@link #unmasked()}.
 *
 * <p><b>Whole lines only.</b> A secret split across two {@code write} calls is still masked, because
 * nothing is written until the line's {@code \n} arrives; {@link #flush} flushes {@code sink} but never
 * a partial line. {@link #close} (and {@link #finish}) write what is left, masked. A secret split
 * across two lines is not masked, as on egret's side (spec §12).
 *
 * <p>Lines are decoded and re-encoded as UTF-8, the encoding egret reads the stream in.
 */
final class MaskingOutputStream extends OutputStream {
    private final OutputStream sink;
    private final ByteArrayOutputStream line = new ByteArrayOutputStream();

    MaskingOutputStream(OutputStream sink) {
        this.sink = sink;
    }

    @Override
    public synchronized void write(int b) throws IOException {
        line.write(b);
        if (b == '\n') {
            emit();
        }
    }

    @Override
    public synchronized void write(byte[] b, int off, int len) throws IOException {
        int start = off;
        int end = off + len;
        for (int i = off; i < end; i++) {
            if (b[i] == '\n') {
                line.write(b, start, i + 1 - start);
                emit();
                start = i + 1;
            }
        }
        if (start < end) {
            line.write(b, start, end - start);
        }
    }

    @Override
    public synchronized void flush() throws IOException {
        sink.flush();
    }

    /** Write the unterminated rest, masked, and flush. */
    synchronized void finish() throws IOException {
        if (line.size() > 0) {
            emit();
        }
        sink.flush();
    }

    /**
     * A stream into the same sink that is NOT masked, for the one line the host leaves as it is: the
     * {@code ::{"outputs": …}::} marker (see {@link EgretPluginHost#outputsLine}). Each write first
     * emits any partial line pending here, masked, so the two keep their order.
     */
    OutputStream unmasked() {
        return new OutputStream() {
            @Override
            public void write(int b) throws IOException {
                write(new byte[] {(byte) b}, 0, 1);
            }

            @Override
            public void write(byte[] b, int off, int len) throws IOException {
                synchronized (MaskingOutputStream.this) {
                    if (line.size() > 0) {
                        emit();
                    }
                    sink.write(b, off, len);
                }
            }

            @Override
            public void flush() throws IOException {
                MaskingOutputStream.this.flush();
            }
        };
    }

    @Override
    public synchronized void close() throws IOException {
        finish();
        sink.close();
    }

    private void emit() throws IOException {
        String text = line.toString(StandardCharsets.UTF_8);
        line.reset();
        sink.write(Masker.mask(text).getBytes(StandardCharsets.UTF_8));
    }
}
