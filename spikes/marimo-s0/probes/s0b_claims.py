"""Single-use check: every worker_claimed pid once, and which are still alive (S0b variant A)."""

import collections
import json
import os
import pathlib
import sys

ROOT = pathlib.Path(__file__).parent.parent
LOG = ROOT / ".work" / "logs" / "timing.jsonl"


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


marks = [json.loads(line) for line in LOG.read_text().splitlines() if line]
claimed = [m["pid"] for m in marks if m["event"] == "worker_claimed"]
counts = collections.Counter(claimed)
result = {
    "label": sys.argv[1] if len(sys.argv) > 1 else "",
    "claimed_pids": claimed,
    "claims": len(claimed),
    "distinct_claimed_pids": len(counts),
    "pids_claimed_more_than_once": [p for p, n in counts.items() if n > 1],
    "claimed_pids_still_alive": [p for p in counts if alive(p)],
    "worker_warm_marks": sum(m["event"] == "worker_warm" for m in marks),
}
print(json.dumps(result))
