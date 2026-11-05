"""Unit tests for compare.py: `python3 -I -m unittest scripts/opa-e2e/test_compare.py`."""
from __future__ import annotations

import importlib.util
import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location("compare", Path(__file__).with_name("compare.py"))
compare = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(compare)

HASH = "2B3B2B9CE842AB8B6A6C614CB1F9604BB8A0D502D1AF49C526B72B10894E95B5"


def run(engine, columns=None, rows=None, error=None, sid="q01", user="ann", scenario="G06", stmt="SELECT id, email FROM lake.s1.t1"):
    return {
        "scenario": scenario, "user": user, "sid": sid, "via": "direct", "engine": engine,
        "statement": stmt, "ok": error is None, "columns": columns or [], "rows": rows or [],
        "error": error,
    }


class CompareTest(unittest.TestCase):
    def go(self, records, explained=""):
        with tempfile.TemporaryDirectory() as d:
            res, exp, rep = Path(d, "r.jsonl"), Path(d, "e.toml"), Path(d, "report.json")
            res.write_text("\n".join(json.dumps(r) for r in records) + "\n")
            exp.write_text(explained)
            out = io.StringIO()
            with redirect_stdout(out):
                code = compare.main([str(res), "--explained", str(exp), "--report", str(rep)])
            return code, json.loads(rep.read_text()), out.getvalue()

    def test_one_masked_column_differs_exits_1_and_names_the_column(self):
        # Arrange: Ranger masks `email` with a different mask than OPA; `id` agrees.
        opa = run("opa", ["id", "email"], [[1, None], [2, None]])
        rgr = run("ranger", ["id", "email"], [[1, "x-1"], [2, "x-2"]])
        # Act
        code, report, out = self.go([opa, rgr])
        # Assert
        self.assertEqual(code, 1)
        self.assertEqual(len(report["unexplained"]), 1)
        self.assertEqual(report["unexplained"][0]["kind"], "values-differ")
        self.assertEqual(report["unexplained"][0]["columns"], ["email"])
        self.assertIn("email", out)

    def test_an_unmasked_column_on_opa_is_never_explained(self):
        opa = run("opa", ["id", "email"], [[1, "a@x"], [2, "b@x"]])
        rgr = run("ranger", ["id", "email"], [[1, HASH], [2, HASH]])
        entry = """
[[difference]]
id = "claims-it"
layer = [2]
mechanism = "m"
right = "egret"
why = "w"
[[difference.layer2]]
scenario = "*"
user = "*"
statement = ".*"
kind = "values-differ"
"""
        code, report, _ = self.go([opa, rgr], entry)
        self.assertEqual(code, 1)
        self.assertEqual([c["kind"] for c in report["forbidden"]], ["opa-clearer-values"])
        self.assertEqual(report["forbidden"][0]["columns"], ["email"])

    def test_more_rows_and_serving_a_refusal_are_forbidden(self):
        records = [
            run("opa", ["region"], [["us"], ["eu"]], sid="a"),
            run("ranger", ["region"], [["us"]], sid="a"),
            run("opa", ["c"], [[1]], sid="b"),
            run("ranger", error="Query x failed: Access Denied: Cannot select", sid="b"),
        ]
        code, report, _ = self.go(records)
        self.assertEqual(code, 1)
        self.assertEqual(sorted(c["kind"] for c in report["forbidden"]), ["opa-more-rows", "opa-serves"])

    def test_a_matching_layer2_entry_explains_and_a_stale_one_fails(self):
        records = [
            run("opa", ["t"], [["t1"]], sid="s", stmt="SHOW TABLES FROM lake.s1"),
            run("ranger", ["t"], [["t1"], ["t2"]], sid="s", stmt="SHOW TABLES FROM lake.s1"),
        ]
        explained = """
[[difference]]
id = "layer-one-only"
operations = ["ShowTables"]
mechanism = "m"
right = "egret"
why = "w"

[[difference]]
id = "hides"
layer = [1, 2]
operations = ["ShowTables"]
mechanism = "m"
right = "egret"
why = "w"
[[difference.layer2]]
scenario = "G0*"
user = "ann"
statement = "^SHOW TABLES"
kind = "opa-hides-rows"
"""
        code, report, _ = self.go(records, explained)
        self.assertEqual(code, 0, report)
        self.assertEqual(report["entries_used"], {"hides": 1})
        stale = explained + """
[[difference]]
id = "never-produced"
layer = [2]
mechanism = "m"
right = "egret"
why = "w"
[[difference.layer2]]
scenario = "G99"
user = "*"
statement = ".*"
kind = "opa-refuses"
"""
        code, report, _ = self.go(records, stale)
        self.assertEqual(code, 1)
        self.assertEqual(report["stale_entries"], ["never-produced"])

    def test_errors_compare_by_class_and_both_sides_are_required(self):
        records = [
            run("opa", error="Query a failed: Access Denied: Cannot select from columns [ssn] in table or view lake.s1.t1", sid="e"),
            run("ranger", error="Query b failed: Access Denied: Cannot select from columns [ssn] in table or view t1", sid="e"),
            run("opa", ["x"], [[1]], sid="lonely"),
        ]
        code, report, _ = self.go(records)
        self.assertEqual(code, 1)
        self.assertEqual(report["tally"], {"same": 1})
        self.assertEqual(report["incomplete"][0]["case"][2], "lonely")

    def test_opa_refusing_what_ranger_serves_is_explainable_but_must_be_explained(self):
        records = [
            run("opa", error="Query a failed: Access Denied: Cannot show columns", sid="r"),
            run("ranger", ["Column"], [["id"]], sid="r"),
        ]
        code, report, _ = self.go(records)
        self.assertEqual(code, 1)
        self.assertEqual(report["unexplained"][0]["kind"], "opa-refuses")


if __name__ == "__main__":
    unittest.main()
