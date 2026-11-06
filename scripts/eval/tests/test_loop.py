import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

import support
from support import STUB_SERVER, FakeLLM, StubFixture, jsonl, read

import agent
import export_hf
import models
import run
import scoring
import tasks
from fixture import Fixture
from mcp_stdio import StdioMcp


def stub(fixture_base, timeout=20):
    m = StdioMcp([sys.executable, STUB_SERVER], env={"STUB_FIXTURE": fixture_base}, timeout=timeout)
    return m.start()


class Script:
    """A test backend that plays a fixed list of turns, recording the messages it was shown."""
    scripted = True
    name = "test-script"

    def __init__(self, turns):
        self.turns = list(turns)
        self.seen = []

    def begin_task(self, task, ctx):
        pass

    def chat(self, system, messages, tools):
        self.seen.append((system, json.loads(json.dumps(messages)), [t["name"] for t in tools]))
        calls = self.turns.pop(0) if self.turns else [("finish", {"final_answer": "out of script", "cited_urls": [], "claim": "unsure"})]
        tc = [{"id": f"c{len(self.seen)}_{i}", "name": n, "args": a} for i, (n, a) in enumerate(calls)]
        return models.Turn("", tc, {"in": 10, "out": 5, "estimated": False})


FIN_OK = ("finish", {"final_answer": "ordered", "cited_urls": [], "claim": "success"})


class AgentLoop(unittest.TestCase):
    def setUp(self):
        self.fx = StubFixture().__enter__()
        self.addCleanup(self.fx.__exit__)

    def run_script(self, turns, **kw):
        m = stub(self.fx.base)
        self.addCleanup(m.close)
        b = Script(turns)
        res = agent.run_agent("do it", self.fx.base + "/order", m, b, **kw)
        return res, b

    def test_real_actions_reach_the_fixture_and_dynamic_tools_are_seen(self):
        res, b = self.run_script([
            [("navigate", {"url": self.fx.base + "/order"})],
            [("submit", {"path": "/order", "body": {"item": "widget", "qty": 2}})],
            [FIN_OK]])
        self.assertTrue(res["finished"])
        self.assertEqual(res["end_reason"], "finished")
        self.assertEqual(res["tool_calls"], 2)
        self.assertEqual((res["tokens_in"], res["tokens_out"], res["tokens_estimated"]), (30, 15, False))
        # submit was not offered on turn 1 and was on turn 2 (tools/list is re-read every turn)
        self.assertNotIn("submit", b.seen[0][2])
        self.assertIn("submit", b.seen[1][2])
        self.assertIn("finish", b.seen[0][2])
        self.assertIn("tools_changed", [e["type"] for e in res["events"]])
        recs = Fixture(self.fx.base).records()
        self.assertTrue(any(r["path"] == "/order" and r["method"] == "POST" for r in recs))

    def test_same_system_prompt_every_turn(self):
        res, b = self.run_script([[("navigate", {"url": self.fx.base})], [FIN_OK]])
        self.assertEqual({s for s, _, _ in b.seen}, {agent.SYSTEM_PROMPT})

    def test_tool_results_are_truncated_identically_and_images_are_placeholders(self):
        res, b = self.run_script([[("big_output", {})], [("image", {})], [FIN_OK]], max_result_chars=100)
        tool_msgs = [m for m in b.seen[-1][1] if m["role"] == "tool"]
        self.assertTrue(tool_msgs[0]["content"].startswith("x" * 100))
        self.assertIn("19900 more characters omitted", tool_msgs[0]["content"])
        self.assertIn("[image omitted by harness: 400 base64 chars]", tool_msgs[1]["content"])
        self.assertIn("a picture", tool_msgs[1]["content"])
        ev = [e for e in res["events"] if e["type"] == "tool"]
        self.assertEqual(ev[0]["result_chars"], 20000)

    def test_truncate_function(self):
        self.assertEqual(agent.truncate("abc", 10), "abc")
        self.assertTrue(agent.truncate("a" * 50, 10).startswith("a" * 10 + "\n[harness"))
        self.assertEqual(agent.truncate("a" * 50, 0), "a" * 50)

    def test_tool_error_and_unknown_tool_do_not_end_the_run(self):
        res, b = self.run_script([[("fail", {})], [("nonexistent", {})], [FIN_OK]])
        self.assertTrue(res["finished"])
        errs = [e["is_error"] for e in res["events"] if e["type"] == "tool"]
        self.assertEqual(errs, [True, True])

    def test_step_limit_gives_no_finish(self):
        res, _ = self.run_script([[("fail", {})]] * 10, max_steps=3)
        self.assertFalse(res["finished"])
        self.assertEqual(res["end_reason"], "step_limit")
        self.assertEqual(res["steps"], 3)

    def test_invalid_claim_is_rejected_and_run_continues(self):
        res, _ = self.run_script([[("finish", {"final_answer": "x", "cited_urls": [], "claim": "kinda"})], [FIN_OK]])
        self.assertTrue(res["finished"])
        self.assertEqual(res["final"]["claim"], "success")
        rejected = [e for e in res["events"] if e["type"] == "tool" and e["name"] == "finish"]
        self.assertEqual(len(rejected), 1)

    def test_text_only_replies_are_nudged_then_given_up_on(self):
        class Chatty(Script):
            def chat(self, system, messages, tools):
                self.seen.append((system, messages, []))
                return models.Turn("I think it is done.", [], {"in": 1, "out": 1, "estimated": True})
        m = stub(self.fx.base)
        self.addCleanup(m.close)
        res = agent.run_agent("do it", "x", m, Chatty([]), max_steps=10)
        self.assertEqual(res["end_reason"], "no_tool_call")
        self.assertFalse(res["finished"])
        self.assertEqual(res["final"]["final_answer"], "I think it is done.")
        self.assertTrue(res["tokens_estimated"])

    def test_tool_timeout_is_reported_not_raised(self):
        m = stub(self.fx.base, timeout=1)
        self.addCleanup(m.close)
        text, is_err, _ = m.call_tool("slow", {"seconds": 3}, timeout=0.5)
        self.assertTrue(is_err)
        self.assertIn("timed out", text)

    def test_finish_name_collision_is_a_harness_error(self):
        class Fin(StdioMcp):
            def list_tools(self):
                return [{"name": "finish", "description": "", "inputSchema": {"type": "object"}}]
        m = Fin([sys.executable, STUB_SERVER], env={"STUB_FIXTURE": self.fx.base})
        m.start()
        self.addCleanup(m.close)
        with self.assertRaises(agent.HarnessError):
            agent.run_agent("x", "y", m, Script([]))


class Scripted(unittest.TestCase):
    def setUp(self):
        self.fx = StubFixture().__enter__()
        self.addCleanup(self.fx.__exit__)
        self.tasks = {t["id"]: t for t in support.sample_tasks()}

    def play(self, spec, tid, seed=0, rep=0):
        backend, _label, _pub = models.build(spec, seed=seed)
        fixture = Fixture(self.fx.base)
        fixture.reset()
        task = self.tasks[tid]
        backend.begin_task(task, {"base": self.fx.base, "fixture": fixture, "rep": rep})
        m = stub(self.fx.base)
        self.addCleanup(m.close)
        res = agent.run_agent(tasks.render(task["instruction"], self.fx.base), "x", m, backend)
        chk = scoring.check_outcome(task["outcome"], fixture.records(), res["final"]["final_answer"],
                                    res["final"]["cited_urls"], self.fx.base)
        return res, chk, scoring.classify(chk["passed"], res["final"]["claim"], res["finished"])

    def test_oracle_passes_records_and_answer_tasks(self):
        for tid in ("order", "read"):
            res, chk, cls = self.play("scripted-oracle", tid)
            self.assertTrue(chk["passed"], (tid, chk))
            self.assertEqual(cls, "true_success")

    def test_oracle_liar_is_a_false_success(self):
        for tid in ("order", "read"):
            _, chk, cls = self.play("scripted-oracle:liar", tid)
            self.assertFalse(chk["passed"])
            self.assertEqual(cls, "false_success")

    def test_oracle_honest_failure_and_unsure(self):
        self.assertEqual(self.play("scripted-oracle:honest-fail", "order")[2], "true_failure")
        self.assertEqual(self.play("scripted-oracle:unsure", "order")[2], "unsure")

    def test_random_is_deterministic_per_seed_task_and_rep(self):
        def trace(seed, rep=0):
            res, _, _ = self.play("scripted-random", "order", seed=seed, rep=rep)
            return [(e["name"], json.dumps(e["args"], sort_keys=True)) for e in res["events"] if e["type"] == "tool"], res["final"]["claim"]
        self.assertEqual(trace(1), trace(1))
        variants = {json.dumps(trace(s)) for s in range(12)}
        self.assertGreater(len(variants), 1)

    def test_random_never_solves_the_order_task(self):
        for s in range(15):
            _, chk, _ = self.play("scripted-random", "order", seed=s)
            self.assertFalse(chk["passed"], f"seed {s} satisfied the check by chance: {chk}")

    def test_unknown_specs_are_errors(self):
        with self.assertRaises(ValueError):
            models.build("gpt-5")
        with self.assertRaises(ValueError):
            models.build("scripted-oracle:magic")
        with self.assertRaisesRegex(ValueError, "base-url"):
            models.build("openai:foo")


class Adapters(unittest.TestCase):
    TOOLS = [{"name": "navigate", "description": "go", "input_schema": {"type": "object", "properties": {"url": {"type": "string"}}}},
             models.FINISH_TOOL]
    MSGS = [{"role": "user", "content": "hi"},
            {"role": "assistant", "text": "", "tool_calls": [{"id": "a", "name": "navigate", "args": {"url": "u"}},
                                                             {"id": "b", "name": "navigate", "args": {"url": "v"}}]},
            {"role": "tool", "id": "a", "name": "navigate", "content": "page A"},
            {"role": "tool", "id": "b", "name": "navigate", "content": "page B"}]

    def test_openai_request_and_response_shapes(self):
        resp = {"choices": [{"message": {"content": None, "tool_calls": [
            {"id": "call_1", "type": "function", "function": {"name": "navigate", "arguments": "{\"url\": \"http://x\"}"}}]},
            "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 123, "completion_tokens": 7}}
        with FakeLLM([resp]) as llm:
            b = models.OpenAICompat("m", llm.base + "/v1", "sk-secret-123456", temperature=0)
            t = b.chat("SYS", self.MSGS, self.TOOLS)
            req = llm.requests[0]
        self.assertEqual(req["path"], "/v1/chat/completions")
        self.assertEqual(req["headers"]["authorization"], "Bearer sk-secret-123456")
        body = req["body"]
        self.assertEqual(body["messages"][0], {"role": "system", "content": "SYS"})
        self.assertEqual(body["messages"][2]["tool_calls"][0]["function"]["arguments"], "{\"url\": \"u\"}")
        self.assertEqual([m["role"] for m in body["messages"]], ["system", "user", "assistant", "tool", "tool"])
        self.assertEqual(body["messages"][3]["tool_call_id"], "a")
        self.assertEqual(body["tools"][0]["function"]["name"], "navigate")
        self.assertEqual(body["temperature"], 0)
        self.assertEqual((t.tool_calls[0]["name"], t.tool_calls[0]["args"]), ("navigate", {"url": "http://x"}))
        self.assertEqual(t.usage, {"in": 123, "out": 7, "estimated": False})

    def test_openai_without_usage_is_estimated_and_bad_arguments_are_flagged(self):
        resp = {"choices": [{"message": {"content": "hm", "tool_calls": [
            {"id": "c", "type": "function", "function": {"name": "navigate", "arguments": "{not json"}}]}}]}
        with FakeLLM([resp]) as llm:
            t = models.OpenAICompat("m", llm.base).chat("S", self.MSGS, self.TOOLS)
        self.assertTrue(t.usage["estimated"])
        self.assertIn("__unparsed_arguments__", t.tool_calls[0]["args"])

    def test_anthropic_request_and_response_shapes(self):
        resp = {"content": [{"type": "text", "text": "ok"}, {"type": "tool_use", "id": "tu1", "name": "finish",
                                                              "input": {"final_answer": "d", "cited_urls": [], "claim": "success"}}],
                "stop_reason": "tool_use", "usage": {"input_tokens": 50, "output_tokens": 9}}
        with FakeLLM([resp]) as llm:
            b = models.AnthropicMessages("claude-x", llm.base, "ak-secret-123456", version="2026-10-21")
            t = b.chat("SYS", self.MSGS, self.TOOLS)
            req = llm.requests[0]
        self.assertEqual(req["path"], "/v1/messages")
        self.assertEqual(req["headers"]["x-api-key"], "ak-secret-123456")
        self.assertEqual(req["headers"]["anthropic-version"], "2026-10-21")
        body = req["body"]
        self.assertEqual(body["system"], "SYS")
        self.assertIn("max_tokens", body)
        self.assertEqual(body["tools"][0]["input_schema"]["type"], "object")
        # both tool results must be in ONE user message after the assistant tool_use blocks
        self.assertEqual([m["role"] for m in body["messages"]], ["user", "assistant", "user"])
        self.assertEqual([c["type"] for c in body["messages"][2]["content"]], ["tool_result", "tool_result"])
        self.assertEqual(body["messages"][2]["content"][0]["tool_use_id"], "a")
        self.assertEqual(t.text, "ok")
        self.assertEqual(t.tool_calls[0]["name"], "finish")
        self.assertEqual(t.usage, {"in": 50, "out": 9, "estimated": False})

    def test_http_error_text_never_leaks_the_key(self):
        with FakeLLM([(401, {"error": "bad key sk-secret-123456"})]) as llm:
            with self.assertRaises(models.ModelError) as cm:
                models.OpenAICompat("m", llm.base, "sk-secret-123456").chat("S", self.MSGS, self.TOOLS)
        self.assertNotIn("sk-secret-123456", str(cm.exception))
        self.assertIn("[REDACTED]", str(cm.exception))

    def test_key_comes_from_the_environment_and_is_not_in_public_config(self):
        os.environ["TEST_EVAL_KEY_X"] = "sk-env-secret-999999"
        try:
            b, _, pub = models.build("openai:foo", base_url="http://127.0.0.1:9/v1", api_key_env="TEST_EVAL_KEY_X")
        finally:
            del os.environ["TEST_EVAL_KEY_X"]
        self.assertEqual(b.key, "sk-env-secret-999999")
        self.assertNotIn("sk-env-secret-999999", json.dumps(pub))
        self.assertEqual(pub["api_key_env"], "TEST_EVAL_KEY_X")


class EndToEnd(unittest.TestCase):
    def test_run_report_and_hf_export(self):
        with StubFixture() as fx, tempfile.TemporaryDirectory() as tmp:
            packs = support.write_packs(os.path.join(tmp, "packs"))
            servers = os.path.join(tmp, "servers.json")
            with open(servers, "w") as f:
                json.dump({"servers": {"stub": {"command": [sys.executable, STUB_SERVER], "env": {"STUB_FIXTURE": fx.base}}}}, f)
            out = os.path.join(tmp, "res")
            buf = io.StringIO()
            with redirect_stdout(buf):
                code = run.main(["--servers-file", servers, "--server", "stub", "--model",
                                 "scripted-oracle,scripted-oracle:liar,scripted-random", "--packs-dir", packs,
                                 "--fixture", fx.base, "--reps", "2", "--out", out])
            self.assertEqual(code, 0, buf.getvalue())
            for name in ("config.json", "tasks.json", "runs.jsonl", "results.json", "results.md"):
                self.assertTrue(os.path.exists(os.path.join(out, name)), name)
            res = json.loads(read(os.path.join(out, "results.json")))
            self.assertEqual(len(res["runs"]), 1 * 3 * 2 * 2)
            by = {(r["model"], r["pack"]): r for r in res["summary"]}
            self.assertEqual(by[("scripted-oracle", "(all)")]["true_success"], 4)
            self.assertEqual(by[("scripted-oracle", "(all)")]["false_success"], 0)
            self.assertEqual(by[("scripted-oracle:liar", "(all)")]["false_success"], 4)
            self.assertAlmostEqual(by[("scripted-oracle:liar", "(all)")]["false_success_rate"], 1.0)
            md = read(os.path.join(out, "results.md"))
            self.assertIn("Scripted backends were used", md)
            self.assertIn("scripted-oracle:liar", md)
            self.assertEqual(res["config"]["server_info"]["stub"]["name"], "stub")
            # one trajectory per run, valid JSONL, ending in a result line
            trajs = os.listdir(os.path.join(out, "trajectories"))
            self.assertEqual(len(trajs), 12)
            lines = jsonl(os.path.join(out, "trajectories", trajs[0]))
            self.assertEqual(lines[0]["type"], "meta")
            self.assertEqual(lines[-1]["type"], "result")
            # fresh fixture state per run: the oracle's records never leak into the next run
            self.assertTrue(all(r["records_seen"] <= 2 for r in res["runs"]))

            hf = os.path.join(tmp, "hf")
            with redirect_stdout(io.StringIO()):
                self.assertEqual(export_hf.main([out, hf]), 0)
            runs = jsonl(os.path.join(hf, "data", "runs.jsonl"))
            traj = jsonl(os.path.join(hf, "data", "trajectories.jsonl"))
            self.assertEqual((len(runs), len(traj)), (12, 12))
            self.assertIsInstance(json.loads(traj[0]["events_json"]), list)
            card = read(os.path.join(hf, "README.md"))
            self.assertTrue(card.startswith("---\nlicense: apache-2.0"))
            self.assertIn("scripted (non-model) runs", card)
            self.assertIn("false success", card)
            self.assertEqual(len(read(os.path.join(hf, "data", "tasks.jsonl")).splitlines()), 2)

    def test_unreachable_fixture_server_is_a_clear_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            packs = support.write_packs(os.path.join(tmp, "packs"))
            err = io.StringIO()
            from contextlib import redirect_stderr
            with redirect_stderr(err):
                code = run.main(["--server", "none", "--model", "scripted-oracle", "--packs-dir", packs,
                                 "--fixture", "http://127.0.0.1:9", "--out", os.path.join(tmp, "o")])
            self.assertEqual(code, 2)
            self.assertIn("ostler fixtures", err.getvalue())

    def test_no_tasks_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            from contextlib import redirect_stderr
            with redirect_stderr(io.StringIO()):
                self.assertEqual(run.main(["--server", "none", "--model", "scripted-oracle", "--packs-dir", tmp]), 2)

    def test_no_key_is_ever_written_to_the_results_dir(self):
        secret = "sk-never-log-this-77777"
        os.environ["TEST_EVAL_KEY_Y"] = secret
        resp = {"choices": [{"message": {"content": None, "tool_calls": [
            {"id": "f", "type": "function", "function": {"name": "finish", "arguments": json.dumps(
                {"final_answer": "order form", "cited_urls": ["{BASE}/order"], "claim": "success"})}}]}}]}
        try:
            with StubFixture() as fx, FakeLLM([resp, resp]) as llm, tempfile.TemporaryDirectory() as tmp:
                packs = support.write_packs(os.path.join(tmp, "packs"), [support.sample_tasks()[1]])
                out = os.path.join(tmp, "res")
                with redirect_stdout(io.StringIO()):
                    code = run.main(["--server", "none", "--model", "openai:fake-model", "--base-url", llm.base + "/v1",
                                     "--api-key-env", "TEST_EVAL_KEY_Y", "--packs-dir", packs, "--fixture", fx.base, "--out", out])
                self.assertEqual(code, 0)
                self.assertEqual(llm.requests[0]["headers"]["authorization"], "Bearer " + secret)
                for root, _, files in os.walk(out):
                    for fn in files:
                        self.assertNotIn(secret, read(os.path.join(root, fn)), fn)
                res = json.loads(read(os.path.join(out, "results.json")))
                self.assertEqual(res["runs"][0]["class"], "true_success")
        finally:
            del os.environ["TEST_EVAL_KEY_Y"]


if __name__ == "__main__":
    unittest.main()
