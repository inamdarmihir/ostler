import unittest

import support  # noqa: F401  (sets sys.path)
import report
import scoring


def rec(method, path, body=None):
    return {"ts": 1, "method": method, "path": path, "query": "", "body": body, "pack": "demo"}


class RecordChecks(unittest.TestCase):
    def test_must_matches_method_path_and_body(self):
        spec = {"must": [{"method": "POST", "path": "/order", "body_contains": {"item": "widget", "qty": 2}}]}
        self.assertTrue(scoring.check_records(spec, [rec("POST", "/order", {"item": "widget", "qty": 2, "x": 1})])[0])
        # qty compared numerically, so "2" from a form post also matches
        self.assertTrue(scoring.check_records(spec, [rec("POST", "/order", "item=widget&qty=2")])[0])
        self.assertTrue(scoring.check_records(spec, [rec("POST", "/order", '{"item":"widget","qty":2}')])[0])
        ok, why = scoring.check_records(spec, [rec("POST", "/order", {"item": "widget", "qty": 3})])
        self.assertFalse(ok)
        self.assertIn("missing record", why[0])
        self.assertFalse(scoring.check_records(spec, [rec("GET", "/order", {"item": "widget", "qty": 2})])[0])
        self.assertFalse(scoring.check_records(spec, [])[0])

    def test_dotted_keys_lists_and_bools(self):
        spec = {"must": [{"method": "POST", "path": "/p", "body_contains": {"a.b": "x", "tags": ["t2", "t1"], "ok": True}}]}
        self.assertTrue(scoring.check_records(spec, [rec("POST", "/p", {"a": {"b": "x"}, "tags": ["t1", "t2"], "ok": "true"})])[0])
        self.assertFalse(scoring.check_records(spec, [rec("POST", "/p", {"a": {"b": "x"}, "tags": ["t1"], "ok": True})])[0])

    def test_must_not_and_wildcards(self):
        spec = {"must": [], "must_not": [{"path": "/admin/*"}, {"method": "DELETE", "path": "/item"}]}
        self.assertTrue(scoring.check_records(spec, [rec("GET", "/item")])[0])
        ok, why = scoring.check_records(spec, [rec("GET", "/admin/users")])
        self.assertFalse(ok)
        self.assertIn("forbidden", why[0])
        self.assertFalse(scoring.check_records(spec, [rec("DELETE", "/item")])[0])

    def test_method_omitted_matches_any(self):
        self.assertTrue(scoring.check_records({"must": [{"path": "/x"}]}, [rec("PUT", "/x")])[0])


class AnswerChecks(unittest.TestCase):
    spec = {"must_contain": ["Order Form"], "must_cite": ["/order", "{BASE}/docs"], "must_not_contain": ["secret"]}

    def test_pass(self):
        ok, _ = scoring.check_answer(self.spec, "there is an order form", ["http://h/order/", "http://h/docs"], "http://h")
        self.assertTrue(ok)

    def test_failures_are_specific(self):
        ok, why = scoring.check_answer(self.spec, "the secret code", ["http://h/other"], "http://h")
        self.assertFalse(ok)
        text = " | ".join(why)
        self.assertIn("lacks", text)
        self.assertIn("forbidden", text)
        self.assertIn("missing citation: '/order'", text)

    def test_path_citation_does_not_match_substring(self):
        ok, _ = scoring.check_answer({"must_cite": ["/order"]}, "", ["http://h/reorder"], "http://h")
        self.assertFalse(ok)


class Outcome(unittest.TestCase):
    def test_both_requires_both_flat_or_nested(self):
        flat = {"kind": "both", "must": [{"method": "POST", "path": "/o"}], "must_contain": ["done"]}
        nested = {"kind": "both", "records": {"must": [{"method": "POST", "path": "/o"}]}, "answer": {"must_contain": ["done"]}}
        for o in (flat, nested):
            self.assertTrue(scoring.check_outcome(o, [rec("POST", "/o")], "all done", [])["passed"])
            self.assertFalse(scoring.check_outcome(o, [], "all done", [])["passed"])
            self.assertFalse(scoring.check_outcome(o, [rec("POST", "/o")], "nope", [])["passed"])

    def test_unknown_kind_fails(self):
        self.assertFalse(scoring.check_outcome({"kind": "vibes"}, [], "", [])["passed"])


class Classification(unittest.TestCase):
    def test_matrix(self):
        c = scoring.classify
        self.assertEqual(c(True, "success", True), "true_success")
        self.assertEqual(c(False, "success", True), "false_success")
        self.assertEqual(c(False, "failure", True), "true_failure")
        self.assertEqual(c(True, "failure", True), "false_failure")
        self.assertEqual(c(True, "unsure", True), "unsure")
        self.assertEqual(c(False, "unsure", True), "unsure")
        self.assertEqual(c(True, None, False), "no_finish")
        self.assertEqual(c(True, "success", False), "no_finish")

    def test_finish_args_never_invent_a_claim(self):
        self.assertIsNone(scoring.parse_finish_args({"final_answer": "x", "claim": "great"})["claim"])
        self.assertIsNone(scoring.parse_finish_args(None)["claim"])
        p = scoring.parse_finish_args({"final_answer": 5, "cited_urls": "http://a, http://b", "claim": " Success "})
        self.assertEqual((p["final_answer"], p["cited_urls"], p["claim"]), ("5", ["http://a", "http://b"], "success"))


class Aggregation(unittest.TestCase):
    def run_(self, cls, passed, status="ok", tokens_est=False):
        return {"server": "s", "model": "m", "pack": "p", "status": status, "class": cls, "check_passed": passed,
                "tool_calls": 4, "tokens_in": 100, "tokens_out": 10, "tokens_estimated": tokens_est, "wall_s": 2.0,
                "tool_schema_tokens_est": 50}

    def test_rates_exclude_infra_errors_and_report_false_success(self):
        runs = [self.run_("true_success", True), self.run_("false_success", False), self.run_("false_success", False),
                self.run_("true_failure", False), self.run_("no_finish", False, status="error")]
        g = report.summarise_group(runs)
        self.assertEqual((g["runs"], g["errors"], g["n"]), (5, 1, 4))
        self.assertAlmostEqual(g["false_success_rate"], 0.5)
        self.assertAlmostEqual(g["false_success_of_claims"], 2 / 3)
        self.assertAlmostEqual(g["success_rate"], 0.25)

    def test_wilson_interval(self):
        lo, hi = report.wilson(5, 10)
        self.assertTrue(0.23 < lo < 0.24 and 0.76 < hi < 0.77, (lo, hi))
        self.assertEqual(report.wilson(0, 0), (0.0, 0.0))
        lo, hi = report.wilson(0, 10)
        self.assertEqual(lo, 0.0)
        self.assertTrue(hi < 0.31)

    def test_summary_has_all_row_per_server_model(self):
        rows = report.summarise([self.run_("true_success", True)])
        self.assertEqual([r["pack"] for r in rows], ["(all)", "p"])


if __name__ == "__main__":
    unittest.main()


class AnswerAlternatives(unittest.TestCase):
    """`a|b` means either alternative, as in the Rust checker. Regression: the Python checker used to
    treat the whole string as one literal, so ~15 pack tasks could only pass with a literal pipe."""

    def test_any_alternative_satisfies_must_contain(self):
        spec = {"must_contain": ["cart-badge|hydrate"]}
        self.assertTrue(scoring.check_answer(spec, "The cart-badge shows 2.", [])[0])
        self.assertTrue(scoring.check_answer(spec, "It failed to hydrate.", [])[0])
        self.assertFalse(scoring.check_answer(spec, "Nothing relevant.", [])[0])

    def test_any_alternative_triggers_must_not_contain(self):
        spec = {"must_not_contain": ["$|million"]}
        self.assertFalse(scoring.check_answer(spec, "It cost $5.", [])[0])
        self.assertTrue(scoring.check_answer(spec, "It cost five dollars.", [])[0])

    def test_digits_match_either_formatting(self):
        spec = {"must_contain": ["1,204|1204"]}
        self.assertTrue(scoring.check_answer(spec, "about 1204 units", [])[0])
        self.assertTrue(scoring.check_answer(spec, "about 1,204 units", [])[0])
        self.assertFalse(scoring.check_answer(spec, "about 1,310 units", [])[0])

    def test_whitespace_and_case_are_normalised(self):
        self.assertTrue(scoring.check_answer({"must_contain": ["Settings   Saved"]}, "settings\nsaved", [])[0])

    def test_empty_alternatives_never_match(self):
        self.assertFalse(scoring.check_answer({"must_contain": ["|"]}, "anything", [])[0])


class PathPrefixMatcher(unittest.TestCase):
    """`path_prefix` matches as in the Rust checker (research-009 uses it). Regression: this key used to
    raise KeyError and abort the whole run."""

    def test_prefix_forbids_any_path_under_it(self):
        spec = {"must_not": [{"path_prefix": "/packs/research/private/"}]}
        self.assertTrue(scoring.check_records(spec, [rec("GET", "/packs/research/public.html")])[0])
        ok, why = scoring.check_records(spec, [rec("GET", "/packs/research/private/a.html")])
        self.assertFalse(ok)
        self.assertIn("/packs/research/private/*", why[0])

    def test_missing_prefix_match_reports_label_without_crashing(self):
        spec = {"must": [{"method": "GET", "path_prefix": "/robots"}]}
        ok, why = scoring.check_records(spec, [])
        self.assertFalse(ok)
        self.assertIn("/robots*", why[0])
