import json
import os
import tempfile
import unittest

import support
import tasks


class Loading(unittest.TestCase):
    def test_load_dict_and_list_forms_and_defaults(self):
        with tempfile.TemporaryDirectory() as d:
            support.write_packs(d)
            os.makedirs(os.path.join(d, "other"))
            with open(os.path.join(d, "other", "tasks.json"), "w") as f:
                json.dump([{"id": "t1", "instruction": "do {BASE}/x", "outcome": {"kind": "answer", "must_contain": ["a"]}}], f)
            ts = tasks.load_packs(d)
            self.assertEqual(sorted(tasks.task_key(t) for t in ts), ["demo/order", "demo/read", "other/t1"])
            t1 = [t for t in ts if t["id"] == "t1"][0]
            self.assertEqual((t1["pack"], t1["tags"], t1["expect_gaps"], t1["title"]), ("other", [], [], "t1"))

    def test_filters(self):
        with tempfile.TemporaryDirectory() as d:
            support.write_packs(d)
            self.assertEqual([t["id"] for t in tasks.load_packs(d, ids=["read"])], ["read"])
            self.assertEqual([t["id"] for t in tasks.load_packs(d, tags=["forms"])], ["order"])
            self.assertEqual(tasks.load_packs(d, packs=["nope"]), [])

    def test_validation_errors_are_specific(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "p"))
            with open(os.path.join(d, "p", "tasks.json"), "w") as f:
                json.dump([{"id": "x", "instruction": "i", "outcome": {"kind": "magic"}}], f)
            with self.assertRaisesRegex(tasks.TaskError, "outcome.kind"):
                tasks.load_packs(d)
            with open(os.path.join(d, "p", "tasks.json"), "w") as f:
                json.dump([{"id": "x", "outcome": {"kind": "answer"}}], f)
            with self.assertRaisesRegex(tasks.TaskError, "instruction"):
                tasks.load_packs(d)
            with open(os.path.join(d, "p", "tasks.json"), "w") as f:
                json.dump([{"id": "x", "instruction": "i", "outcome": {"kind": "records", "must": [{"method": "POST"}]}}], f)
            with self.assertRaisesRegex(tasks.TaskError, "path"):
                tasks.load_packs(d)

    def test_duplicate_ids_in_a_pack_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            t = support.sample_tasks()[1]
            support.write_packs(d, [t, dict(t)])
            with self.assertRaisesRegex(tasks.TaskError, "duplicate"):
                tasks.load_packs(d)

    def test_render_and_start_url(self):
        self.assertEqual(tasks.render("go {BASE}/a", "http://h:1/"), "go http://h:1/a")
        self.assertEqual(tasks.start_url({"start": "/order"}, "http://h:1"), "http://h:1/order")
        self.assertEqual(tasks.start_url({"start": "{BASE}/x"}, "http://h:1"), "http://h:1/x")
        self.assertEqual(tasks.start_url({"start": "https://e.com/z"}, "http://h:1"), "https://e.com/z")
        self.assertEqual(tasks.start_url({}, "http://h:1"), "http://h:1/")


if __name__ == "__main__":
    unittest.main()
