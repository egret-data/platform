package io.kestra.core.runners;

/**
 * Thrown by every {@link ShimRunContext} collaborator egret does not provide: internal storage,
 * the KV store, namespace files, Kestra's secret key, its application context.
 *
 * <p>Anywhere in the cause chain of what a plugin throws, it makes the host exit 4, which egret
 * reports as "needs internal storage (W9)" rather than as a plugin failure.
 */
public class EgretUnsupported extends RuntimeException {
    public EgretUnsupported(String what) {
        super(what);
    }
}
