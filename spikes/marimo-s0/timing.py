"""Append-only timing marks for the S0b breakdown (spike only)."""

import json
import os
import time


def mark(event: str, **kw) -> None:
    path = os.environ.get("S0_TIMING_LOG")
    if not path:
        return
    line = json.dumps({"t": time.time(), "pid": os.getpid(), "event": event, **kw})
    with open(path, "a") as f:
        f.write(line + "\n")
