#!/usr/bin/env python3
"""W3a execution oracle -- traces.json -> golden.json.

`harvest.py --traces 5` writes the reduced raw execution documents.  This turns
them into the NORMALISED SEMANTIC TRACE that `tests/executor.rs` compares egret
against, and writes `stability.json`, the measured record of what had to be
dropped for not reproducing.

    python3 normalise.py              # write golden.json + stability.json
    python3 normalise.py --self-test  # mutate the committed data, prove every guard fires

Nothing here talks to Kestra: golden.json is a pure function of the two
committed files, for the same reason `harvest.py --rewrite-runnable` is.  A
defect in the normalisation must be fixable without a two-hour re-harvest, or
the pressure is to hand-edit the golden -- the one file whose whole worth is
that nobody edits it.

The rules are in README.md under "The normalised trace".  In short: ordinals
instead of generated ids, `taskRunList` order kept, history state sequences kept
verbatim, every clock dropped -- but only AFTER the happens-before relation has
been derived from it, and only for the pairs that came out the same in all five
rounds.
"""

import argparse
import copy
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))

# The two rejection classes that are evidence egret must refuse a flow, and the
# two that are not.  Task 1 measured the split and `runnable.txt` prints it:
# `task-type-not-in-image` and `checkout-drift` are the pinned image and the
# sibling checkout differing, NOT Kestra judging a flow invalid.  A golden that
# demanded a refusal for those would manufacture 23 defects out of a packaging
# accident -- so they get no golden entry at all, and the count is published.
REFUSAL_EVIDENCE = ("invalid-flow", "refused-by-both")
NOT_REFUSAL_EVIDENCE = ("task-type-not-in-image", "checkout-drift")

# `before` and `after` are the claim Kestra's own tests make in
# `assertThat(a2.startDate.isAfter(a1.endDate)).isTrue()` -- FOUR times, in
# ParallelTest.errors() and DagTest.errors(), counted at checkout 96893b589 and
# not the 39 the plan claims.  It is a claim about SERIALISATION, not about how
# long anything took, and those four are the only assertions in Kestra's suite
# that separate `parallel` and `dag` from "the same tasks in some order".
# `overlapping` is its negation and is just as much a measurement: a `parallel`
# whose branches stop overlapping has stopped being parallel.
RELATIONS = ("before", "after", "overlapping")


def bail(msg):
    """Guards raise; they never `assert`.

    `python3 -O` strips assert statements, so a guard written as an assert is a
    guard that disappears under a flag anyone may pass.  Same rule as
    harvest.py's.
    """
    raise SystemExit("normalise.py guard failed: " + msg)


# --------------------------------------------------------------------------
# one execution -> one normalised node
# --------------------------------------------------------------------------
def history_states(state):
    """Rule 4: the history state sequence, verbatim.

    Verbatim includes `SUBMITTED`, which the executor's draft `RunState` does
    not have, and includes the repeated `CREATED` a retried task shows.  Both
    are load-bearing: SUBMITTED is the only thing in the record that separates a
    task the WORKER ran from a flowable the EXECUTOR ran, and dropping it would
    be normalising away a distinction rather than a representation.
    """
    return [h[0] for h in (state or {}).get("histories") or []]


def relation(a, b):
    """`before` / `after` / `overlapping` for two runs, or None if unmeasurable.

    Strictly after, matching the Java form `a2.startDate.isAfter(a1.endDate)`:
    two runs that share a timestamp are `overlapping`, never `before`.  ISO-8601
    UTC strings out of Kestra compare correctly as strings -- same zone, same
    shape, fixed-width fields -- and parsing them would only add a way to be
    wrong.  A run with no end (created but never started) yields None, which is
    dropped and counted separately from the pairs dropped for instability: "not
    measurable" and "measured differently five times" are different facts.
    """
    a_start, a_end = a["state"].get("startDate"), a["state"].get("endDate")
    b_start, b_end = b["state"].get("startDate"), b["state"].get("endDate")
    if not (a_start and a_end and b_start and b_end):
        return None
    if a_end < b_start:
        return "before"
    if b_end < a_start:
        return "after"
    return "overlapping"


def ordered_runs(doc):
    """The execution's taskruns in the order the normaliser numbers them.

    That is the order each run was CREATED, which is `taskRunList` order with
    one correction measured on `valids/working-directory-real-runif-false`.
    There, a `SKIPPED` child and the sibling after it are created about 200
    microseconds apart by the same executor pass, and `taskRunList` records
    them in whichever order the write landed: in rounds 4 and 5 the sibling came
    first in the list, though its `CREATED` instant is later than the skipped
    child's in all five rounds. Numbering by the list therefore moved the
    skipped run's ordinal from round to round, which made the whole trace
    unstable and kept the entry out of the golden. Numbering by the `CREATED`
    instant reads the same five rounds as one order.

    Ties (equal `CREATED` instants) keep the `taskRunList` position, and the
    harvest measured no tie at microsecond resolution, so for every other corpus
    flow this is the same order `taskRunList` gives -- `golden.json`'s other
    entries are unchanged by it, and `stability.json` says so per flow.
    """
    def created(r):
        histories = (r.get("state") or {}).get("histories") or []
        if not histories:
            bail(f"taskrun {r.get('taskId')!r} ({r.get('id')}) has no CREATED history, "
                 "so it has no creation instant to number it by")
        return histories[0][1]

    keyed = [(created(r), i, r) for i, r in enumerate(doc["runs"])]
    return [r for _, _, r in sorted(keyed, key=lambda k: (k[0], k[1]))]


def ordinals(doc):
    """taskrun id -> its ordinal: its index in `ordered_runs`.

    Rule 1 says ordinal in FIRST-APPEARANCE order and rule 3 says keep the
    creation order; those are the same order only if no taskrun's parent appears
    after it.  That is checked rather than assumed -- see `check_parent_links`.
    """
    return {r["id"]: i for i, r in enumerate(ordered_runs(doc))}


def check_parent_links(cid, doc):
    """A parent taskrun must be resolvable, and must appear EARLIER in the list.

    Two things this catches, and one it had to be taught.

    Taught: **a `parentTaskRunId` can cross the execution boundary.** Measured on
    `invalids/loop-switch-failed` -- a LOOP sub-execution's top-level taskruns
    carry the PARENT execution's loop taskrun as their `parentTaskRunId`
    (`switch` and `errorforeach` both name `foreach`, which lives in the root's
    `taskRunList`).  Inside this execution they have no parent, and the link they
    are expressing is the one `parent_run` already carries.  So an id equal to
    this execution's `spawnedBy` normalises to `null`, and anything else is still
    a hard failure -- the exemption is exactly one id wide, not "not found means
    null".

    Caught: rule 1 says ordinal in FIRST-APPEARANCE order and rule 3 says keep
    `taskRunList` order.  Those are the same numbering only if no taskrun's
    parent appears after it, and if they ever were not, the golden would silently
    be using whichever one this file happened to implement.
    """
    ix = ordinals(doc)
    outer = doc.get("spawnedBy")
    for i, r in enumerate(ordered_runs(doc)):
        p = r.get("parentTaskRunId")
        if p is None or p == outer:
            continue
        if p not in ix:
            bail(f"{cid}: taskrun {r['taskId']!r} names parentTaskRunId {p} "
                 f"which is neither in the same taskRunList nor this execution's "
                 f"spawnedBy ({outer})")
        if ix[p] >= i:
            bail(f"{cid}: taskrun {r['taskId']!r} at index {i} has its parent at "
                 f"index {ix[p]}; taskRunList order is then NOT first-appearance "
                 "order and rules 1 and 3 disagree")


def node_runs(doc):
    ix = ordinals(doc)
    outer = doc.get("spawnedBy")
    out = []
    for r in ordered_runs(doc):
        p = r.get("parentTaskRunId")
        out.append({
            "task": r["taskId"],
            "value": r.get("value"),
            "parent": None if p is None or p == outer else ix[p],
            "state": history_states(r["state"]),
            "attempts": [history_states(a) for a in r.get("attempts") or []],
        })
    return out


def spawn_key(child, parent_doc):
    """Where a sub-execution sorts: by the parent run, then by its LOOP INDEX.

    Rule 5 says `spawned` is ordered by the parent run.  Within one parent run a
    loop spawns several, and the tiebreak has to be semantic:

    - ksuids are not usable -- a ksuid orders at one-second resolution, so
      siblings created in the same second sort at random;
    - the creation instant is not usable either, and this is MEASURED, not
      feared: ordering by `startDate` made nine corpus flows fail the five-round
      stability check with the same set of sub-executions in a different
      sequence (`loop-object`, `loop-object-in-list`, `if-in-parallel`,
      `switch-in-concurrent-loop`, ...).  A concurrent loop creates its
      iterations in whatever order the scheduler picks, which is a fact about
      the scheduler and not about the flow.

    `loopRun.index` is the flow's own numbering of its iterations and is what
    `Command::SpawnExecution` will be judged on.  The instant survives only as a
    last resort for a sub-execution that carries no index, and is dropped from
    the output like every other clock.
    """
    ix = ordinals(parent_doc)
    by = child.get("spawnedBy")
    index = child.get("loopIndex")
    return (ix.get(by, -1) if by is not None else -1,
            index if index is not None else -1,
            child["state"].get("startDate") or "",
            child["id"])


def normalise_node(cid, doc, by_parent, path, orders):
    """One execution and everything under it, minus the order pairs.

    `orders` is filled in as a side table keyed by PATH -- "" for the root,
    "0/1" for the second sub-execution of the first -- because the pairs still
    have to survive the five-round stability filter before they may be written
    into the trace.
    """
    check_parent_links(cid, doc)
    runs = node_runs(doc)

    pairs = {}
    ordered = ordered_runs(doc)
    for i in range(len(ordered)):
        for j in range(i + 1, len(ordered)):
            pairs[f"{i},{j}"] = relation(ordered[i], ordered[j])
    orders[path] = pairs

    ix = ordinals(doc)
    children = sorted(by_parent.get(doc["id"], []), key=lambda c: spawn_key(c, doc))
    seen_keys = set()
    for child in children:
        key = spawn_key(child, doc)[:2]
        if key[1] >= 0 and key in seen_keys:
            # Two iterations of one loop claiming the same index: the ordering
            # would then be settled by the creation instant after all, silently,
            # for exactly the flows the index was introduced to fix.
            bail(f"{cid}: two sub-executions of run ordinal {key[0]} both carry "
                 f"loop index {key[1]}; the spawned order is not determined")
        seen_keys.add(key)
    spawned = []
    for k, child in enumerate(children):
        by = child.get("spawnedBy")
        sub = normalise_node(cid, child, by_parent, f"{path}/{k}" if path else str(k), orders)
        sub["value"] = child.get("loopValue")
        if by is not None and by not in ix:
            # `loopRun.parent.taskRunList` is the PARENT execution's list, so a
            # spawnedBy that is not in it means the link was read off the wrong
            # document -- and a wrong parent link reads exactly like a measured
            # one once the ids are gone.
            bail(f"{cid}: sub-execution {child['id']} says it was spawned by "
                 f"taskrun {by}, which is not in its parent's taskRunList")
        sub["parent_run"] = ix[by] if by is not None else None
        spawned.append(sub)

    return {"execution": history_states(doc["state"]), "runs": runs, "spawned": spawned}


def normalise_round(cid, rec):
    """One captured round -> (trace-without-order, {path: {pair: relation}})."""
    root, descendants = rec["root"], rec["descendants"]
    by_parent = {}
    for d in descendants:
        by_parent.setdefault(d.get("parentId"), []).append(d)

    reachable = set()

    def walk(node_id):
        reachable.add(node_id)
        for c in by_parent.get(node_id, []):
            walk(c["id"])

    walk(root["id"])
    orphans = [d["id"] for d in descendants if d["id"] not in reachable]
    if orphans:
        # A sub-execution whose parentId points outside the tree is one the
        # trace would silently omit -- the exact failure `descendants()` unions
        # two discoveries to avoid, arriving one layer later.
        bail(f"{cid}: {len(orphans)} captured sub-execution(s) do not hang off the "
             f"root, e.g. {orphans[:3]}")

    orders = {}
    trace = normalise_node(cid, root, by_parent, "", orders)
    return trace, orders


# --------------------------------------------------------------------------
# five rounds -> one entry
# --------------------------------------------------------------------------
def stable_orders(per_round, rounds):
    """Keep a pair only where all `rounds` rounds agree; return the drops too.

    A relation observed once is a coincidence.  This is the same discipline the
    creation-order experiment used, and it is the only thing that makes `order`
    worth asserting on: without it a genuinely parallel pair lands in the golden
    as whichever way it happened to fall the first time, and Task 4 would be
    judged against a coin toss.
    """
    kept, unstable, unmeasurable = {}, [], []
    paths = set()
    for orders in per_round:
        paths |= set(orders)
    for path in sorted(paths):
        present = [o.get(path) for o in per_round]
        if any(p is None for p in present):
            # The tree shape itself moved between rounds; the trace-level
            # stability check reports that, and no pair under it is trustworthy.
            continue
        for pair in sorted(present[0], key=lambda k: tuple(int(x) for x in k.split(","))):
            seen = [p.get(pair) for p in present]
            if any(r is None for r in seen):
                unmeasurable.append([path, pair, seen])
                continue
            if len(set(seen)) == 1:
                kept.setdefault(path, {})[pair] = seen[0]
            else:
                unstable.append([path, pair, seen])
    return kept, unstable, unmeasurable


def attach_orders(node, kept, path=""):
    """Write the surviving pairs into the trace, and settle the key order.

    Field order is `execution, runs, order, spawned` at every depth, plus
    `parent_run` first on a sub-execution -- the shape the plan writes and the
    shape `tests/executor.rs` reads.  A golden nobody can read by eye is a
    golden nobody checks by eye.
    """
    order = [[int(p.split(",")[0]), int(p.split(",")[1]), rel]
             for p, rel in sorted(kept.get(path, {}).items(),
                                  key=lambda kv: tuple(int(x) for x in kv[0].split(",")))]
    spawned = [attach_orders(sub, kept, f"{path}/{k}" if path else str(k))
               for k, sub in enumerate(node["spawned"])]
    out = {}
    if "parent_run" in node:
        out["parent_run"] = node["parent_run"]
        out["value"] = node.get("value")
    out["execution"] = node["execution"]
    out["runs"] = node["runs"]
    out["order"] = order
    out["spawned"] = spawned
    return out


def build(traces, corpus):
    """golden.json + stability.json, from the two committed files."""
    rounds = traces["rounds"]
    by_id = {r["id"]: r for r in corpus}

    entries, unstable_traces, stability = [], [], {}

    for cid in sorted(traces["flows"]):
        per_round_traces, per_round_orders = [], []
        for rec in traces["flows"][cid]:
            trace, orders = normalise_round(cid, rec)
            per_round_traces.append(trace)
            per_round_orders.append(orders)

        first = per_round_traces[0]
        differing = [i + 1 for i, t in enumerate(per_round_traces) if t != first]
        kept, unstable, unmeasurable = stable_orders(per_round_orders, rounds)
        stability[cid] = {
            "rounds_differing_from_round_1": differing,
            "pairs_kept": sum(len(v) for v in kept.values()),
            "pairs_dropped_unstable": len(unstable),
            "pairs_dropped_unmeasurable": len(unmeasurable),
            "unstable": unstable,
            "unmeasurable": unmeasurable,
        }
        if differing:
            # Excluded from the golden, and PUBLISHED.  A trace the oracle itself
            # does not reproduce five times running cannot judge egret: whichever
            # round were written down, an implementation matching it would be
            # right by luck and an implementation matching another round would be
            # marked wrong.  Silently keeping round 1 is the version of this that
            # looks fine.
            unstable_traces.append(cid)
            continue

        trace = attach_orders(copy.deepcopy(first), kept)
        entries.append({
            "id": cid,
            "verdict": "runs",
            "execution": trace["execution"],
            "runs": trace["runs"],
            "order": trace["order"],
            "spawned": trace["spawned"],
        })

    for rec in corpus:
        if rec.get("disposition") != "rejected":
            continue
        cls = rec.get("rejected_class")
        if cls in REFUSAL_EVIDENCE:
            entries.append({"id": rec["id"], "verdict": "refused", "rejected_class": cls})
        elif cls not in NOT_REFUSAL_EVIDENCE:
            bail(f"{rec['id']} is `rejected` with class {cls!r}, which is neither "
                 f"evidence {list(REFUSAL_EVIDENCE)} nor non-evidence "
                 f"{list(NOT_REFUSAL_EVIDENCE)}; a class nobody has classified must "
                 "not silently become a golden entry or silently not become one")

    entries.sort(key=lambda e: e["id"])
    report = {
        "rounds": rounds,
        "image": traces["image"],
        "unstable_traces": sorted(unstable_traces),
        "flows": stability,
    }
    check(entries, report, traces, corpus, by_id)
    return entries, report


# --------------------------------------------------------------------------
# guards
# --------------------------------------------------------------------------
def check(entries, report, traces, corpus, by_id):
    # NOT `len(refused) == the corpus count of refusal-evidence rejections`:
    # both sides of that come from iterating one list once, which is the shape
    # of the tautology that survived four mutations in Task 1.  This compares
    # two INDEPENDENT observations -- what Kestra refused at publish, and what
    # Kestra executed to a terminal state -- and they must be disjoint.  It is
    # first because it is the most specific thing that can be said about an id
    # in both sets; "duplicate id" below is true of it too, and less useful.
    both = sorted({e["id"] for e in entries if e["verdict"] == "refused"}
                  & set(traces["flows"]))
    if both:
        bail(f"{len(both)} flow(s) have a `refused` golden entry AND a captured "
             f"execution trace, e.g. {both[:5]}; a flow cannot both be refused and "
             "run, so one of the two artifacts is describing a different corpus")

    ids = [e["id"] for e in entries]
    if len(ids) != len(set(ids)):
        dupes = sorted({i for i in ids if ids.count(i) > 1})
        bail(f"golden.json would carry duplicate id(s) {dupes[:5]}")

    runs_ids = {r["id"] for r in corpus if r.get("disposition") == "runs"}
    if set(traces["flows"]) != runs_ids:
        bail(f"traces.json covers {len(traces['flows'])} flows but corpus.json has "
             f"{len(runs_ids)} with disposition `runs`")

    # Every corpus flow must land in exactly one of: a golden entry, or a named
    # exclusion.  The arithmetic is the point -- a flow that is in none of them
    # is a flow nobody compares and nobody counts, which is how seven flows once
    # went invisible at exit 0.
    accounted = {e["id"] for e in entries}
    accounted |= set(report["unstable_traces"])
    accounted |= {r["id"] for r in corpus if r.get("disposition") == "not-standalone"}
    accounted |= {r["id"] for r in corpus
                  if r.get("rejected_class") in NOT_REFUSAL_EVIDENCE}
    missing = sorted({r["id"] for r in corpus} - accounted)
    if missing:
        bail(f"{len(missing)} corpus flow(s) are neither in golden.json nor in a "
             f"named exclusion, e.g. {missing[:5]}")

    for e in entries:
        if e["verdict"] == "refused":
            continue
        if not e["execution"]:
            bail(f"{e['id']}: golden entry has an empty execution history; the "
                 "capture is broken, not the flow")
        for i, j, rel in e["order"]:
            if rel not in RELATIONS:
                bail(f"{e['id']}: order pair [{i},{j}] has relation {rel!r}, which "
                     f"is not one of {list(RELATIONS)}")
        for r in e["runs"]:
            if r["parent"] is not None and not (0 <= r["parent"] < len(e["runs"])):
                bail(f"{e['id']}: run {r['task']!r} has parent ordinal "
                     f"{r['parent']}, out of range")



# --------------------------------------------------------------------------
def load():
    with open(os.path.join(HERE, "traces.json"), encoding="utf-8") as f:
        traces = json.load(f)
    with open(os.path.join(HERE, "corpus.json"), encoding="utf-8") as f:
        corpus = json.load(f)
    return traces, corpus


def write(entries, report):
    with open(os.path.join(HERE, "golden.json"), "w", encoding="utf-8") as f:
        json.dump(entries, f, indent=1, sort_keys=False)
        f.write("\n")
    with open(os.path.join(HERE, "stability.json"), "w", encoding="utf-8") as f:
        json.dump(report, f, indent=1, sort_keys=True)
        f.write("\n")


def summarise(entries, report, corpus):
    runs = sum(1 for e in entries if e["verdict"] == "runs")
    refused = sum(1 for e in entries if e["verdict"] == "refused")
    # Over the flows that REACHED the golden, not over every flow harvested.
    # `runnable.txt`'s table has the same denominator, and two published pair
    # counts that differ by the excluded flows' pairs is exactly the kind of
    # one-off nobody can explain six months later.
    graded = [f for cid, f in report["flows"].items()
              if cid not in set(report["unstable_traces"])]
    kept = sum(f["pairs_kept"] for f in graded)
    uns = sum(f["pairs_dropped_unstable"] for f in graded)
    unm = sum(f["pairs_dropped_unmeasurable"] for f in graded)
    excl_ns = sum(1 for r in corpus if r.get("disposition") == "not-standalone")
    excl_rj = sum(1 for r in corpus if r.get("rejected_class") in NOT_REFUSAL_EVIDENCE)
    print(f"[golden]  runs {runs}  refused {refused}  TOTAL {len(entries)}")
    print(f"[excluded] not-standalone {excl_ns}  non-evidence rejections {excl_rj}  "
          f"unstable traces {len(report['unstable_traces'])}  "
          f"TOTAL {excl_ns + excl_rj + len(report['unstable_traces'])}")
    print(f"[order]   kept {kept}  dropped unstable {uns}  dropped unmeasurable {unm}  "
          f"TOTAL {kept + uns + unm}")
    if report["unstable_traces"]:
        print("[unstable] " + ", ".join(report["unstable_traces"]))


def self_test():
    """Prove each guard FIRES, by mutating the committed data.

    Same rule as harvest.py's self-test, for the same reason: a guard nobody has
    seen fail is a guard nobody has seen.  A previous guard in this package was
    `assert sum(by_class.values()) == len(rej)`, both sides of which came from
    iterating one list once; it survived four mutations.
    """
    traces, corpus = load()

    def must_bail(name, fn):
        try:
            fn()
        except SystemExit as e:
            print(f"  FIRES   {name}\n            {str(e)[:150]}")
            return True
        print(f"  SILENT  {name}  <-- the guard did not fire")
        return False

    print("[self-test] every guard below must fire on its mutation")
    ok = True

    # 1. traces.json short of corpus.json's `runs` set
    m = copy.deepcopy(traces)
    m["flows"].pop(sorted(m["flows"])[0])
    ok &= must_bail("traces.json missing a `runs` flow",
                    lambda: build(m, corpus))

    # 2. a rejection class that is neither evidence nor non-evidence
    mc = copy.deepcopy(corpus)
    next(r for r in mc if r.get("rejected_class") == "invalid-flow")[
        "rejected_class"] = "looks-fine"
    ok &= must_bail("a rejected class outside both vocabularies",
                    lambda: build(traces, mc))

    # 3. a taskrun whose parent appears after it -- rules 1 and 3 disagreeing.
    #    The numbering is by CREATED instant (`ordered_runs`), so the mutation
    #    has to move a child's creation instant before its parent's: reversing
    #    the list no longer changes the numbering, which is the point of
    #    numbering by instant.
    m = copy.deepcopy(traces)
    cid = next(c for c, rs in sorted(m["flows"].items())
               if any(r.get("parentTaskRunId") for r in rs[0]["root"]["runs"]))
    rs = m["flows"][cid][0]["root"]["runs"]
    ids = {r["id"] for r in rs}
    child = next(r for r in rs if r.get("parentTaskRunId") in ids)
    child["state"]["histories"][0][1] = "0000-01-01T00:00:00Z"
    ok &= must_bail("a parent taskrun that appears after its child",
                    lambda: build(m, corpus))

    # 4. a taskrun naming a parent that is not in the list at all
    m = copy.deepcopy(traces)
    cid = next(c for c, rs in sorted(m["flows"].items())
               if any(r.get("parentTaskRunId") for r in rs[0]["root"]["runs"]))
    next(r for r in m["flows"][cid][0]["root"]["runs"]
         if r.get("parentTaskRunId"))["parentTaskRunId"] = "not-a-taskrun"
    ok &= must_bail("a parentTaskRunId that names no taskrun",
                    lambda: build(m, corpus))

    # 5. a sub-execution that does not hang off the root
    m = copy.deepcopy(traces)
    cid = next(c for c, rs in sorted(m["flows"].items()) if rs[0]["descendants"])
    m["flows"][cid][0]["descendants"][0]["parentId"] = "somewhere-else"
    ok &= must_bail("an orphaned sub-execution",
                    lambda: build(m, corpus))

    # 6. an execution captured with no history at all.  The flow has to be one
    #    that REACHES the golden: mutating an unstable one only proves that
    #    unstable ones are excluded, which is a different guard.
    entries, _ = build(traces, corpus)
    stable = next(e["id"] for e in entries if e["verdict"] == "runs")
    m = copy.deepcopy(traces)
    for r in m["flows"][stable]:
        r["root"]["state"]["histories"] = []
    ok &= must_bail("a golden entry with an empty execution history",
                    lambda: build(m, corpus))

    # 7. the accounting: a corpus flow in no golden entry and no named exclusion.
    #    Its disposition moves to one that is neither compared nor excluded --
    #    NOT to `runs`, which the traces-coverage guard would catch first and
    #    would prove the wrong thing.
    mc = copy.deepcopy(corpus)
    next(r for r in mc if r.get("disposition") == "not-standalone")[
        "disposition"] = "unclassified"
    ok &= must_bail("a corpus flow accounted for by nothing",
                    lambda: build(traces, mc))

    # 8. a flow that both ran and was refused -- the two artifacts disagreeing
    mc = copy.deepcopy(corpus)
    r = next(r for r in mc if r.get("disposition") == "runs"
             and r["id"] in traces["flows"])
    r["disposition"], r["rejected_class"] = "rejected", "invalid-flow"
    ok &= must_bail("a flow that is both refused and captured running",
                    lambda: build(traces, mc))

    # 9. two iterations of one loop claiming the same index -- the tiebreak the
    #    spawned order rests on, silently ambiguous
    m = copy.deepcopy(traces)
    cid = next((c for c, rs in sorted(m["flows"].items())
                if len([d for d in rs[0]["descendants"] if d.get("loopIndex") is not None]) > 1),
               None)
    if cid is None:
        print("  SILENT  duplicate loop index  <-- no flow in traces.json carries one")
        ok = False
    else:
        subs = [d for d in m["flows"][cid][0]["descendants"] if d.get("loopIndex") is not None]
        subs[1]["loopIndex"] = subs[0]["loopIndex"]
        subs[1]["spawnedBy"] = subs[0]["spawnedBy"]
        ok &= must_bail("two loop iterations claiming the same index",
                        lambda: build(m, corpus))

    if not ok:
        raise SystemExit("[self-test] at least one guard did not fire")
    print("[self-test] all guards fired")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--self-test", action="store_true",
                    help="mutate the committed data and prove every guard fires")
    args = ap.parse_args()

    if args.self_test:
        self_test()
        return

    traces, corpus = load()
    entries, report = build(traces, corpus)
    write(entries, report)
    summarise(entries, report, corpus)


if __name__ == "__main__":
    main()
