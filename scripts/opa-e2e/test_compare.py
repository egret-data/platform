"""Unit tests for compare.py.

Run: `python3 -I -m unittest discover -s scripts/opa-e2e -p test_compare.py` (with `-I` the
working directory is off `sys.path`, so the file-path form of unittest cannot import it).
"""
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
DENIED = "PERMISSION_DENIED"


def run(engine, columns=None, rows=None, error=None, sid="q01", user="ann", scenario="G06",
        stmt="SELECT id, email FROM lake.s1.t1", error_name=None, harness=False):
    return {
        "scenario": scenario, "user": user, "sid": sid, "via": "direct", "engine": engine,
        "statement": stmt, "ok": error is None, "columns": columns or [], "rows": rows or [],
        "error": error, "error_name": error_name if error is not None else None, "harness": harness,
    }


def entry(eid, *matchers, layer="[2]"):
    out = f'[[difference]]\nid = "{eid}"\nlayer = {layer}\nmechanism = "m"\nright = "egret"\nwhy = "w"\n'
    if layer != "[2]":
        out += 'operations = ["ShowTables"]\n'
    for sc, user, stmt, kind in matchers:
        out += (f'[[difference.layer2]]\nscenario = "{sc}"\nuser = "{user}"\n'
                f"statement = '{stmt}'\nkind = \"{kind}\"\n")
    return out + "\n"


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

    # --- the brief's case -------------------------------------------------------------------
    def test_one_masked_column_differs_exits_1_and_names_the_column(self):
        # Arrange: OPA masks `email` to NULL, Ranger with a CUSTOM mask; `id` agrees.
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

    # --- the never-explainable rule on every path (I2) --------------------------------------
    def test_an_unmasked_column_on_opa_is_never_explained(self):
        opa = run("opa", ["id", "email"], [[1, "a@x"], [2, "b@x"]])
        rgr = run("ranger", ["id", "email"], [[1, HASH], [2, HASH]])
        claims = entry("claims-it", ("*", "*", ".*", "values-differ"))
        code, report, _ = self.go([opa, rgr], claims)
        self.assertEqual(code, 1)
        self.assertEqual([c["kind"] for c in report["forbidden"]], ["opa-clearer-values"])
        self.assertEqual(report["forbidden"][0]["columns"], ["email"])
        self.assertEqual(report["forbidden"][0]["claimed_by"], ["claims-it"])

    def test_a_distinctness_keeping_mask_dropped_on_opa_is_caught_by_the_reference(self):
        # CUSTOM concat('x-', email) on Ranger, clear on OPA: neither NULL nor hex, same count of
        # distinct values; only the policy-free reference tells which side is clear.
        opa = run("opa", ["email"], [["a@x"], ["b@x"]])
        rgr = run("ranger", ["email"], [["x-a@x"], ["x-b@x"]])
        ref = run("ref", ["email"], [["a@x"], ["b@x"]])
        code, report, _ = self.go([opa, rgr, ref], entry("c", ("*", "*", ".*", "values-differ")))
        self.assertEqual(code, 1)
        self.assertEqual([c["kind"] for c in report["forbidden"]], ["opa-clearer-values"])

    def test_opa_masking_more_than_ranger_is_not_forbidden(self):
        opa = run("opa", ["email"], [["x-a@x"], ["x-b@x"]])
        rgr = run("ranger", ["email"], [["a@x"], ["b@x"]])
        ref = run("ref", ["email"], [["a@x"], ["b@x"]])
        code, report, _ = self.go([opa, rgr, ref])
        self.assertEqual(code, 1)
        self.assertEqual(report["forbidden"], [])
        self.assertEqual(report["unexplained"][0]["kind"], "values-differ")

    def test_a_hidden_column_does_not_hide_a_clear_one(self):
        # OPA drops `salary` (opa-hides-columns) but returns `email` clear where Ranger masks it.
        opa = run("opa", ["id", "email"], [[1, "a@x"], [2, "b@x"]])
        rgr = run("ranger", ["id", "email", "salary"], [[1, "x-a@x", 1], [2, "x-b@x", 2]])
        ref = run("ref", ["id", "email", "salary"], [[1, "a@x", 1], [2, "b@x", 2]])
        claims = entry("c", ("*", "*", ".*", "opa-hides-columns"))
        code, report, _ = self.go([opa, rgr, ref], claims)
        self.assertEqual(code, 1)
        self.assertEqual([(c["kind"], c["columns"]) for c in report["forbidden"]],
                         [("opa-clearer-values", ["email"])])

    def test_fewer_rows_do_not_hide_a_clear_value(self):
        # OPA filters harder (rows-differ) but unmasks what Ranger masks.
        opa = run("opa", ["id", "ssn"], [[2, "000000002"]])
        rgr = run("ranger", ["id", "ssn"], [[1, "XXXXX0001"], [2, "XXXXX0002"]])
        ref = run("ref", ["id", "ssn"], [[1, "000000001"], [2, "000000002"]])
        claims = entry("c", ("*", "*", ".*", "rows-differ"))
        code, report, _ = self.go([opa, rgr, ref], claims)
        self.assertEqual(code, 1)
        self.assertEqual([c["kind"] for c in report["forbidden"]], ["opa-clearer-values"])

    def test_a_row_ranger_filters_out_is_never_explained(self):
        # Same count, OPA returns a clear row Ranger's filter removes.
        opa = run("opa", ["region"], [["us"], ["eu"]])
        rgr = run("ranger", ["region"], [["us"], ["us"]])
        ref = run("ref", ["region"], [["us"], ["eu"], ["us"]])
        code, report, _ = self.go([opa, rgr, ref], entry("c", ("*", "*", ".*", "values-differ")))
        self.assertEqual([c["kind"] for c in report["forbidden"]], ["opa-clearer-values"])

    def test_more_rows_more_columns_and_serving_a_refusal_are_forbidden(self):
        records = [
            run("opa", ["region"], [["us"], ["eu"]], sid="a"),
            run("ranger", ["region"], [["us"]], sid="a"),
            run("opa", ["c"], [[1]], sid="b"),
            run("ranger", error="Access Denied: Cannot select", error_name=DENIED, sid="b"),
            run("opa", ["a", "b"], [[1, 2]], sid="c"),
            run("ranger", ["a"], [[1]], sid="c"),
        ]
        code, report, _ = self.go(records)
        self.assertEqual(code, 1)
        self.assertEqual(sorted(c["kind"] for c in report["forbidden"]),
                         ["opa-more-columns", "opa-more-rows", "opa-serves"])

    # --- explained and stale ---------------------------------------------------------------
    def test_a_matching_layer2_matcher_explains_and_an_unused_one_is_stale(self):
        records = [
            run("opa", ["t"], [["t1"]], sid="s", stmt="SHOW TABLES FROM lake.s1"),
            run("ranger", ["t"], [["t1"], ["t2"]], sid="s", stmt="SHOW TABLES FROM lake.s1"),
        ]
        layer_one = ('[[difference]]\nid = "layer-one-only"\noperations = ["ShowTables"]\n'
                     'mechanism = "m"\nright = "egret"\nwhy = "w"\n\n')
        good = layer_one + entry("hides", ("G0*", "ann", "^SHOW TABLES", "opa-hides-rows"), layer="[1, 2]")
        code, report, _ = self.go(records, good)
        self.assertEqual(code, 0, report)
        self.assertEqual(report["entries_used"], {"hides": 1})
        # A second matcher of the same, used entry that no case produces is stale on its own.
        stale = layer_one + entry("hides", ("G0*", "ann", "^SHOW TABLES", "opa-hides-rows"),
                                  ("G99", "*", ".*", "opa-refuses"), layer="[1, 2]")
        code, report, _ = self.go(records, stale)
        self.assertEqual(code, 1)
        self.assertEqual(len(report["stale_matchers"]), 1)
        self.assertIn("layer2[1]", report["stale_matchers"][0])

    def test_opa_hiding_every_row_needs_its_own_kind(self):
        records = [
            run("opa", ["t"], [], sid="s", stmt="SHOW TABLES FROM lake.s1"),
            run("ranger", ["t"], [["t1"]], sid="s", stmt="SHOW TABLES FROM lake.s1"),
        ]
        code, report, _ = self.go(records, entry("hides", ("*", "*", ".*", "opa-hides-rows")))
        self.assertEqual(code, 1)
        self.assertEqual(report["unexplained"][0]["kind"], "opa-hides-all-rows")

    # --- errors (I1) -----------------------------------------------------------------------
    def test_denials_compare_by_name_and_normalised_message(self):
        records = [
            run("opa", error="Access Denied: Cannot select from columns [name, region, email, ssn] in table or view lake.s1.t1", error_name=DENIED, sid="e"),
            run("ranger", error="Access Denied: Cannot select from columns [ssn] in table or view t1", error_name=DENIED, sid="e"),
            run("opa", error="Access Denied: User ann cannot impersonate user bob", error_name=DENIED, sid="i"),
            run("ranger", error="Access Denied: Principal ann cannot become user bob", error_name=DENIED, sid="i"),
        ]
        code, report, _ = self.go(records)
        self.assertEqual(code, 0, report)
        self.assertEqual(report["tally"], {"same": 2})

    def test_two_other_errors_are_not_the_same(self):
        records = [
            run("opa", error="Table 'lake.s2.t3_v2' does not exist", error_name="TABLE_NOT_FOUND", sid="a"),
            run("ranger", error="Something else broke", error_name="GENERIC_INTERNAL_ERROR", sid="a"),
            run("opa", error="Access Denied: Cannot show tables of schema lake.s2", error_name=DENIED, sid="b"),
            run("ranger", error="Access Denied: Cannot access catalog lake", error_name=DENIED, sid="b"),
        ]
        code, report, _ = self.go(records)
        self.assertEqual(code, 1)
        self.assertEqual([c["kind"] for c in report["unexplained"]], ["errors-differ", "errors-differ"])

    def test_a_harness_error_fails_even_when_both_sides_have_it(self):
        records = [
            run("opa", error="layer2 harness: no response", error_name="HARNESS", harness=True, sid="h"),
            run("ranger", error="layer2 harness: no response", error_name="HARNESS", harness=True, sid="h"),
        ]
        code, report, out = self.go(records)
        self.assertEqual(code, 1)
        self.assertEqual(len(report["harness_errors"]), 2)
        self.assertIn("HARNESS", out)

    def test_a_case_with_one_side_is_incomplete(self):
        code, report, _ = self.go([run("opa", ["x"], [[1]], sid="lonely")])
        self.assertEqual(code, 1)
        self.assertEqual(report["incomplete"][0]["case"][2], "lonely")

    def test_opa_refusing_what_ranger_serves_is_explainable_but_must_be_explained(self):
        records = [
            run("opa", error="Access Denied: Cannot show columns", error_name=DENIED, sid="r"),
            run("ranger", ["Column"], [["id"]], sid="r"),
        ]
        code, report, _ = self.go(records)
        self.assertEqual(code, 1)
        self.assertEqual(report["unexplained"][0]["kind"], "opa-refuses")


if __name__ == "__main__":
    unittest.main()
