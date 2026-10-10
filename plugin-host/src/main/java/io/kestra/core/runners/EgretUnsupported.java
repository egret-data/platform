package io.kestra.core.runners;

/**
 * Thrown by every {@link ShimRunContext} collaborator egret does not provide: a plugin's KV store
 * (2026-10-07, W9c: the KV store is the {@code core.kv} tasks' and {@code kv()}'s, not the
 * RunContext's), Kestra's secret key, its application context. (Namespace files are provided since W9b, 2026-10-06:
 * {@code storage().namespace(ns)} is {@link EgretNamespace}, whose writes throw
 * {@link UnsupportedOperationException}, a plugin failure, not this.)
 *
 * <p>Anywhere in the cause chain of what a plugin throws, it makes the host exit 4, which egret
 * reports as "a Kestra service egret does not provide to plugins" rather than as a plugin failure.
 */
public class EgretUnsupported extends RuntimeException {
    public EgretUnsupported(String what) {
        super(what);
    }
}
