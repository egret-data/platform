#!/usr/bin/env bash
# Keep cargo's `deps/` directory small enough that starting a binary stays instant.
#
# THE MEASUREMENT THIS EXISTS FOR (this repo, 2026-08-25). Starting a test binary that
# lives in `target/debug/deps` — running ZERO tests — cost:
#
#     6,479 files ->  0.04 s        35,704 files ->  0.16 s        940,602 files -> 34.6 s
#
# with 0.00 s of user and system time in the process itself. The same binary copied to
# /tmp started in 0.03 s at every one of those moments, so the cost is the DIRECTORY, not
# the binary. It degrades as roughly n^1.65 — 26x the files cost 216x the time.
#
# The cost is per PROCESS, so it multiplies by the number of test binaries: at 940k files
# a 21-binary suite paid ~12 minutes before running a single assertion.
#
# WHY A COUNT CAP AND NOT AN AGE CUTOFF: when this was found, every one of those 940,602
# files was under three days old. They are ordinary build output — cargo simply never
# garbage-collects `deps/`. "Delete things older than N days" would have freed nothing.
#
# WHY NOT CLEAN EVERY RUN: a full workspace rebuild is 10.3 minutes and produces ~35,700
# files. Cleaning to save 35 seconds loses 17x on the trade.
#
# SECOND DIRECTORY, DIFFERENT PROBLEM (added 2026-08-26). `debug/incremental` does not
# affect startup at all — it is capped for DISK. Measured the day this was added: `target`
# had reached 93 G, of which `incremental` alone was 37 G across ~650 session directories
# (~50 MB each), because cargo never garbage-collects it either. It had never been pruned
# because the original script only knew about `deps/`.
#
# It is capped by DIRECTORY COUNT rather than by bytes on purpose: `du` on a 37 G tree
# walks every file and takes seconds, which is too slow for something that runs on every
# commit, while `ls -f | wc -l` on the session directories is one cheap readdir.
#
# Deleting a session directory only costs the next build's incremental speed for the
# crates it covered; it is never a correctness risk, unlike deleting from `deps/`.
set -euo pipefail

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
# ~2.2 full workspace builds' worth. Above this, per-binary startup passes ~0.6 s and a
# full suite starts paying tens of seconds of dead time.
MAX_FILES="${PRUNE_MAX_FILES:-80000}"
MIN_AGE_MIN="${PRUNE_MIN_AGE_MIN:-60}"
# ~650 session directories was 37 G. 200 keeps it near 10 G while leaving plenty of
# incremental state for the crates anyone is actively editing.
MAX_INCREMENTAL_DIRS="${PRUNE_MAX_INCREMENTAL_DIRS:-200}"

for deps in "$TARGET_DIR"/debug/deps "$TARGET_DIR"/release/deps; do
  [ -d "$deps" ] || continue

  # Never delete an artifact out from under a running build: the failure that produces
  # looks exactly like a source error and costs an afternoon to diagnose.
  if pgrep -f "cargo (build|test|clippy|run|check)" >/dev/null 2>&1; then
    echo "prune-target: a cargo build is running — leaving $deps alone"
    continue
  fi

  count=$(ls -f "$deps" | wc -l | tr -d ' ')
  if [ "$count" -le "$MAX_FILES" ]; then
    echo "prune-target: $deps has $count files (cap $MAX_FILES) — nothing to do"
    continue
  fi

  excess=$((count - MAX_FILES))
  echo "prune-target: $deps has $count files, over the $MAX_FILES cap."
  echo "prune-target: removing the $excess oldest — startup cost grows ~n^1.65 past this."
  find "$deps" -type f -mmin +"$MIN_AGE_MIN" -print0 \
    | xargs -0 stat -f '%m %N' 2>/dev/null \
    | sort -n | head -n "$excess" | cut -d' ' -f2- \
    | tr '\n' '\0' | xargs -0 rm -f 2>/dev/null || true
  echo "prune-target: $deps now has $(ls -f "$deps" | wc -l | tr -d ' ') files"
done

for inc in "$TARGET_DIR"/debug/incremental "$TARGET_DIR"/release/incremental; do
  [ -d "$inc" ] || continue

  # Same guard as above, and for the same reason: a build that loses its incremental
  # state mid-run reports errors that look like source errors.
  if pgrep -f "cargo (build|test|clippy|run|check)" >/dev/null 2>&1; then
    echo "prune-target: a cargo build is running — leaving $inc alone"
    continue
  fi

  # `-mindepth 1 -maxdepth 1`: count SESSION DIRECTORIES, not the files inside them.
  dirs=$(find "$inc" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
  if [ "$dirs" -le "$MAX_INCREMENTAL_DIRS" ]; then
    echo "prune-target: $inc has $dirs session dirs (cap $MAX_INCREMENTAL_DIRS) — nothing to do"
    continue
  fi

  excess=$((dirs - MAX_INCREMENTAL_DIRS))
  echo "prune-target: $inc has $dirs session dirs, over the $MAX_INCREMENTAL_DIRS cap."
  echo "prune-target: removing the $excess oldest — this costs rebuild speed, never correctness."
  find "$inc" -mindepth 1 -maxdepth 1 -type d -print0 \
    | xargs -0 stat -f '%m %N' 2>/dev/null \
    | sort -n | head -n "$excess" | cut -d' ' -f2- \
    | tr '\n' '\0' | xargs -0 rm -rf 2>/dev/null || true
  echo "prune-target: $inc now has $(find "$inc" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ') session dirs"
done
