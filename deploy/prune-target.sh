#!/usr/bin/env bash
# Keep cargo's `deps/` directory small enough that starting a binary stays instant,
# and keep `incremental/` from filling the disk.
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
# affect startup at all. It was added because `target` had reached 93 G, of which
# `incremental` alone was 37 G across ~650 session directories (~50 MB each), because
# cargo never garbage-collects it either.
#
# ⚠ WHAT THE DIRECTORY CAP ACTUALLY BOUNDS, AND WHAT IT DOES NOT (corrected 2026-10-10,
# SP4d debt Task 17). This header used to say `incremental` "is capped for DISK". It is
# not: `MAX_INCREMENTAL_DIRS` bounds a COUNT of session directories, and a count is not a
# bound on bytes. ~50 MB was the average session directory at the moment it was measured,
# so 200 dirs was *about* 10 G that day; the per-directory size is whatever rustc happens
# to write, so the same 200 dirs can be 2 G or 40 G. Tasks 8+10 of this plan proved the
# gap the expensive way: `/` reached 100 % with 117 Mi available while this cap was in
# force and reporting itself satisfied, and recovery meant deleting 7.3 G of
# `debug/incremental` by hand. The task after that then lost time to a repair, because the
# full disk had filled the Docker VM's log with `input/output error` and left Postgres
# unreachable.
#
# The count cap's REASONING is still sound for its own job — `du` on a 37 G tree walks
# every file and takes seconds, which is too slow for something that runs on every commit,
# while `ls -f | wc -l` is one cheap readdir. So the byte signal added here is NOT `du`:
# it is `df` on the volume, which is O(1) and reports the quantity that actually matters
# — FREE SPACE, not tree size. A tree can be huge on a disk with room to spare, and that
# is not a problem; 117 Mi free is a problem whatever the tree measures.
#
# So there are now two independent caps on `incremental`, and they answer different
# questions:
#
#   MAX_INCREMENTAL_DIRS  — count. Stops unbounded accumulation during normal work, and
#                           only while no cargo/rustc process is running: for a COUNT cap
#                           the build guard below is evaluated first.
#   PRUNE_MIN_FREE_GB     — bytes, via `df`. Fires when the VOLUME is running out, and
#                           then drops the count cap to LOW_DISK_INCREMENTAL_DIRS. It is
#                           evaluated BEFORE the build guard and OUTSIDE the
#                           `[ -d "$inc" ]` test, so a crossed floor prunes `incremental`
#                           even mid-build and an absent `incremental` still reports the
#                           volume. The next paragraph is why that inversion is deliberate.
#
# ⚠ WHY THE FLOOR IS CHECKED BEFORE THE BUILD GUARD FOR `incremental` AND NOT FOR `deps/`
# (fixed 2026-10-10, whole-branch review finding H-A). When the floor was added it was
# written UNDERNEATH the build guard, inherited from the `deps/` loop above without being
# re-decided, and that made it unable to fire in the one scenario it exists for. Measured
# on 2026-10-10 on the unfixed script, on a throwaway fixture tree of three session
# directories with a real cargo build in flight: `PRUNE_MIN_FREE_GB=999999` deleted 0 of
# 3, exited 0, and never printed a free-space figure at all — the floor was never
# evaluated. The control, identical but with `pgrep` stubbed to exit 1, dropped the cap to
# 0 and deleted 3 of 3. A disk that fills WHILE something builds is the only way this disk
# has ever filled, so "fires only when the volume is running out" was promising something
# the code could not do.
#
# The two directories need OPPOSITE answers because their failure modes are opposite.
# Deleting from `deps/` mid-build can break that build, and a broken build is recoverable
# in minutes — so for `deps/` erring toward not deleting is right and the guard stays
# first. For `incremental`, NOT deleting IS the disk-full, and the disk-full on this host
# did not merely break a build: `/` reached 117 Mi, which filled the Docker VM's log with
# `input/output error` and left Postgres unreachable for an hour (74 test failures, every
# binary ~3.0 s — the shape of a connection timeout). So on a crossed floor the floor wins.
#
# The inversion is not free and the cost is named rather than denied: removing a crate's
# incremental directory while rustc is writing into it can make that build fail, and the
# failure reads as a source error. It is NARROWED, not eliminated — while a build is
# running the low-disk path skips any crate directory that currently holds an
# `s-*-working` session directory, rustc's marker for a session it has not finalised.
# rustc can still create one between that check and the `rm`, so this is a smaller race,
# not a closed one. At 117 Mi free, losing one build is the better side of that bet.
#
# Outside that one case, deleting a FINALISED session directory only costs the next
# build's incremental speed for the crates it covered; it is never a correctness risk,
# unlike deleting from `deps/`. That asymmetry is why the low-disk path is allowed to
# empty `incremental` outright and is NOT allowed to go harder on `deps/`.
#
# ⚠ THIS SCRIPT PRUNES ONE TREE: the one it was invoked for ($CARGO_TARGET_DIR, or
# ./target). Several worktrees share this host and other sessions build in them — on
# 2026-10-01 there were three holding 161 G of `target` between them; on 2026-10-10 there
# are FOUR worktrees (`egret`, `egret-sp4a`, `egret-w7`, `egret-wt-marimo-s0`), of which
# two held a `target` of that size — `egret` at 73 G and `egret-sp4a` at 32 G. The earlier
# version of this line said "two", counting target directories and calling them worktrees;
# the two populations are not the same and the enumeration below walks the worktrees.
# (Those numbers are statements about a moment: re-measure, do not cite them.)
# The tempting fix for a full disk is a sweep across all of them, and it is the wrong one:
# this script runs from a post-commit hook in whichever worktree committed, with no
# knowledge of what the others are mid-build on, and deleting another tree's `deps/` while
# its build runs produces errors that read as source errors. When the volume is low the
# low-disk path therefore NAMES the sibling worktrees and leaves them completely alone.
set -euo pipefail

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
# ~2.2 full workspace builds' worth. Above this, per-binary startup passes ~0.6 s and a
# full suite starts paying tens of seconds of dead time.
MAX_FILES="${PRUNE_MAX_FILES:-80000}"
MIN_AGE_MIN="${PRUNE_MIN_AGE_MIN:-60}"
# ~650 session directories was 37 G the day this was measured. 200 kept it near 10 G
# THAT day while leaving plenty of incremental state for the crates anyone is actively
# editing. It is a count, not a size — see the header.
MAX_INCREMENTAL_DIRS="${PRUNE_MAX_INCREMENTAL_DIRS:-200}"
# The byte cap. Below this much free space on the volume holding $TARGET_DIR, the
# incremental cap drops to $LOW_DISK_INCREMENTAL_DIRS. 20 GiB is chosen against two
# measurements: one full workspace rebuild of this repo writes well over 10 G into
# `target`, and the disk-full that motivated this left 117 Mi — so the threshold has to
# be high enough that a build which starts just after a pass cannot cross zero before
# the next commit fires this script again. Evaluated once, before the build guard — see
# the header's H-A paragraph.
MIN_FREE_GB="${PRUNE_MIN_FREE_GB:-20}"
# 0 = remove every session directory in THIS tree. Safe by the asymmetry above: the only
# cost is the next build's incremental speed.
LOW_DISK_INCREMENTAL_DIRS="${PRUNE_LOW_DISK_INCREMENTAL_DIRS:-0}"

# Available space in GiB on the volume holding $1. O(1) — one statfs, no tree walk,
# which is the whole point (see the header's `du` note). `-P` forces POSIX one-line
# output so a long device name cannot wrap and shift the column; field 4 is Available
# in 1024-byte blocks on both macOS and Linux. `df` the TARGET DIR and not `/`: on macOS
# `/` is the sealed system snapshot and reports a different volume than the data volume
# `target` actually lives on.
#
# The trailing `|| true` is load-bearing under `set -o pipefail`: without it a failing
# `df` makes this pipeline non-zero, the `free_before=$(free_gb …)` assignment inherits
# that, and `set -e` kills the script *before* the warning below can say the floor is
# unenforced. Measured while writing this: with a stub `df` that exits 1, the whole
# incremental loop was skipped and the run exited 1 having printed only the deps line.
free_gb() {
  df -Pk "$1" 2>/dev/null | awk 'NR==2 { printf "%d", $4 / 1048576 }' || true
}

# Never delete an artifact out from under a running build: the failure that produces
# looks exactly like a source error and costs an afternoon to diagnose.
#
# ⚠ `pgrep -x`, NOT `pgrep -f "cargo (build|test|…)"` (fixed 2026-10-10, SP4d debt
# Task 17). `-f` matches the FULL COMMAND LINE of every process, so it fired on any
# shell whose command line merely contained the words — measured on this host: a
# `/bin/zsh -c … cat > crates/egret-workflow/tests/w7a_t1_sizes.rs <<EOF …` heredoc from
# another agent session matched it, and this script printed "a cargo build is running"
# and pruned nothing while no cargo process existed at all (`pgrep -x cargo` exited 1 at
# the same instant). A guard that can be held shut for hours by an unrelated shell is
# how a byte cap gets added and still never fires. `-x` matches the executable NAME, so
# it sees cargo and rustc themselves and nothing that merely talks about them.
#
# It is still deliberately GLOBAL rather than per-worktree: pgrep cannot see a process's
# cwd portably, so a build in a sibling tree does pause pruning here. That errs toward not
# deleting, which is the right direction for `deps/` — and was the WRONG direction for
# `incremental`, where not deleting is the disk-full. Review H-A found this guard had been
# inherited to `incremental` unchanged and was what held the byte cap shut; the floor there
# is now evaluated above it. Do not "fix" that by weakening this function back to `-f`.
build_running() {
  pgrep -x cargo >/dev/null 2>&1 || pgrep -x rustc >/dev/null 2>&1
}

for deps in "$TARGET_DIR"/debug/deps "$TARGET_DIR"/release/deps; do
  [ -d "$deps" ] || continue

  if build_running; then
    echo "prune-target: a cargo/rustc process is running — leaving $deps alone"
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

# ── THE BYTE FLOOR, evaluated ONCE, before the build guard and outside `[ -d "$inc" ]` ──
#
# Once rather than per-directory: `debug/incremental` and `release/incremental` are on the
# same volume by construction, so a second `df` would answer the same question twice.
#
# On a path that EXISTS. The floor used to be computed inside `[ -d "$inc" ]` (review
# H-A), so a tree with no `incremental` directory never evaluated it and a volume at 117 Mi
# went unreported by a script whose whole job that day was to report it. `$TARGET_DIR` is
# the right probe for the reason the header's `df`-not-`du` note gives, and it falls back
# to the tree's parent and then `.` — both of which a post-commit hook always has — so an
# empty figure below means `df` FAILED, never "the path was missing".
disk_probe="$TARGET_DIR"
if [ ! -d "$disk_probe" ]; then
  disk_probe=$(dirname "$TARGET_DIR")
  [ -d "$disk_probe" ] || disk_probe="."
fi
free_before=$(free_gb "$disk_probe")
low_disk=no
# Say so out loud, in all three states. A byte cap that silently degrades to "no byte cap"
# when its measurement fails is the same defect this gate was added to close — and so is
# one that says nothing at all when it does not fire, because then "it never mentioned
# free space" is indistinguishable from "it was never reached", which is exactly how H-A
# survived review.
if [ -z "$free_before" ]; then
  echo "prune-target: WARNING — df gave no free-space figure for $disk_probe; the ${MIN_FREE_GB} GiB floor is NOT being enforced on this run."
elif [ "$free_before" -lt "$MIN_FREE_GB" ]; then
  low_disk=yes
  echo "prune-target: ${free_before} GiB free on $disk_probe's volume, under the ${MIN_FREE_GB} GiB floor."
  # Name the siblings; do NOT touch them. See the header's one-tree note. Sizes are
  # left to the operator because `du` on these is exactly the cost `df` avoids.
  # `|| true` on the whole pipeline, and `if` rather than `&&` inside it: with
  # `set -e` a trailing `&&`-list that ends false aborts the script, and when that
  # happened here the sibling NOTICE silently replaced the pruning it was supposed
  # to accompany (the run printed the names, freed nothing, and exited 1).
  if git rev-parse --git-dir >/dev/null 2>&1; then
    mine=$(cd "$(dirname "$TARGET_DIR")" 2>/dev/null && pwd -P || true)
    git worktree list --porcelain 2>/dev/null \
      | awk '/^worktree /{print $2}' \
      | while read -r wt; do
          here=$(cd "$wt" 2>/dev/null && pwd -P || true)
          if [ -n "$here" ] && [ "$here" = "$mine" ]; then
            continue
          fi
          if [ -d "$wt/target" ]; then
            echo "prune-target: NOT pruned (another session may be building): $wt/target"
          fi
        done || true
  fi
else
  echo "prune-target: ${free_before} GiB free on $disk_probe's volume, at or above the ${MIN_FREE_GB} GiB floor — the byte cap does not fire."
fi

# Is rustc still inside this crate directory? An unfinalised session is `s-<id>-working`;
# rustc renames it to `s-<id>` when it is done, so the suffix is the marker and not a
# heuristic about mtimes. One readdir per candidate, and only consulted on the low-disk
# path WHILE a build runs, so the ordinary no-build prune pays nothing for it.
has_working_session() {
  [ -n "$(find "$1" -mindepth 1 -maxdepth 1 -type d -name 's-*-working' -print -quit 2>/dev/null)" ]
}

for inc in "$TARGET_DIR"/debug/incremental "$TARGET_DIR"/release/incremental; do
  [ -d "$inc" ] || continue

  cap="$MAX_INCREMENTAL_DIRS"
  protect_working=no
  if [ "$low_disk" = yes ]; then
    cap="$LOW_DISK_INCREMENTAL_DIRS"
    echo "prune-target: dropping the session-dir cap from $MAX_INCREMENTAL_DIRS to $cap for THIS tree's $inc."
    if build_running; then
      # The H-A inversion: the floor beats the guard here. See the header.
      protect_working=yes
      echo "prune-target: a cargo/rustc process is running, but the floor is crossed — pruning $inc anyway, skipping crate dirs with an unfinalised s-*-working session."
    fi
  elif build_running; then
    # Floor not crossed, so the COUNT cap is the only one asking, and for a count cap
    # waiting for the build is the right direction — see the header.
    echo "prune-target: a cargo/rustc process is running — leaving $inc alone"
    continue
  fi

  # `-mindepth 1 -maxdepth 1`: count SESSION DIRECTORIES, not the files inside them.
  dirs=$(find "$inc" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
  if [ "$dirs" -le "$cap" ]; then
    echo "prune-target: $inc has $dirs session dirs (cap $cap, ${free_before:-?} GiB free) — nothing to do"
    continue
  fi

  excess=$((dirs - cap))
  echo "prune-target: $inc has $dirs session dirs, over the $cap cap."
  echo "prune-target: removing the $excess oldest — this costs rebuild speed, and correctness only for a build this takes the session dir of (header, H-A)."
  # Oldest first. The removal is a loop rather than the one-shot `head | xargs rm` this
  # used to be, because a skipped in-use directory must not consume one of the $excess
  # slots: the loop walks further down the list instead of freeing less than it reported.
  candidates=$(find "$inc" -mindepth 1 -maxdepth 1 -type d -print0 \
    | xargs -0 stat -f '%m %N' 2>/dev/null \
    | sort -n | cut -d' ' -f2- || true)
  removed=0
  skipped=0
  while IFS= read -r victim; do
    [ "$removed" -lt "$excess" ] || break
    [ -n "$victim" ] || continue
    if [ "$protect_working" = yes ] && has_working_session "$victim"; then
      skipped=$((skipped + 1))
      continue
    fi
    rm -rf "$victim" 2>/dev/null || true
    removed=$((removed + 1))
  done <<CANDIDATES
$candidates
CANDIDATES
  if [ "$skipped" -gt 0 ]; then
    echo "prune-target: left $skipped crate dir(s) alone — an unfinalised s-*-working session says a build is inside them."
  fi
  free_after=$(free_gb "$disk_probe")
  echo "prune-target: $inc now has $(find "$inc" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ') session dirs, ${free_after:-?} GiB free (was ${free_before:-?} GiB)"
done
