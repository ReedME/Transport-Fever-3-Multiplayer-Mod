"""Tests of tools/lane_diff.py: python tools/test_lane_diff.py"""

import io
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import lane_diff  # noqa: E402

JAMES = """\
[1759240000] the room says: Diverged { step: 250, lanes: [3] }
[1759240010] dumping lanes 3 at the checkpoint after step 300 (step 250 diverged): lines "lane <n> step 300"
[1759240010] lane 3 step 300 vehicle-0 state=1 stop=0 line=line-0 edge=3 pos=10.199999999999999 speed=5 entity=401 row=1:0:3@10.20 v5.00
[1759240010] lane 3 step 300 vehicle-1 state=2 stop=1 line=nil edge=0 pos=0 speed=0 entity=402 row=2:1:0@0.00 v0.00
[1759240010] lane 3 step 300 summary 2:0000000001-0000000002
[1759240010] lane dump at step 300: lanes 3, 3 entries written
"""


class LaneDiffTest(unittest.TestCase):
    def logs(self, **games):
        folder = Path(tempfile.mkdtemp())
        args = []
        for name, text in games.items():
            path = folder / f"{name}.log"
            path.write_text(text, encoding="utf-8")
            args.append(f"{name}={path}")
        return args

    def run_diff(self, *args):
        out = io.StringIO()
        code = lane_diff.main(list(args), out)
        return code, out.getvalue()

    def test_logs_that_agree_say_so(self):
        code, text = self.run_diff(*self.logs(james=JAMES, bob=JAMES))
        self.assertEqual(code, 0)
        self.assertIn("lane 3 (vehicles) step 300: james, bob: all 2 entries agree", text)
        self.assertIn("the lane's text agrees", text)

    def test_economy_summary_is_not_mistaken_for_a_row_count(self):
        economy = """\
lane 4 step 300 player balance=1234567 row=25:1234567
lane 4 step 300 summary 25:1234567
lane dump at step 300: lanes 4, 2 entries written
"""
        code, text = self.run_diff(*self.logs(james=economy, bob=economy))
        self.assertEqual(code, 0, text)
        self.assertIn("all 1 entries agree", text)
        self.assertIn("the lane's text agrees", text)

    def test_different_summaries_are_shown_even_when_entries_match(self):
        economy = """\
lane 4 step 300 player balance=1234567 row=25:1234567
lane 4 step 300 summary 25:1234567
lane dump at step 300: lanes 4, 2 entries written
"""
        bob = economy.replace("summary 25:1234567", "summary 25:1234568")
        code, text = self.run_diff(*self.logs(james=economy, bob=bob))
        self.assertEqual(code, 1, text)
        self.assertIn("all 1 entries agree; the lane's text differs", text)
        self.assertIn("summary james: 25:1234567", text)
        self.assertIn("summary bob: 25:1234568", text)

    def test_the_differing_vehicle_is_named_with_the_fields_that_differ(self):
        bob = JAMES.replace("pos=0 speed=0", "pos=0.001 speed=0")
        cat = JAMES.replace("state=2 stop=1", "state=3 stop=2").replace(
            "row=2:1:0", "row=3:2:0").replace("summary 2:0000000001", "summary 2:0000000009")
        code, text = self.run_diff(*self.logs(james=JAMES, bob=bob, cat=cat))
        self.assertEqual(code, 1)
        self.assertIn("1 of 2 entries differ; the lane's text differs", text)
        self.assertIn("  vehicle-1\n", text)
        self.assertIn("    james: state=2 stop=1 pos=0 row=2:1:0@0.00 v0.00\n", text)
        self.assertIn("    bob: state=2 stop=1 pos=0.001 row=2:1:0@0.00 v0.00\n", text)
        self.assertIn("    cat: state=3 stop=2 pos=0 row=3:2:0@0.00 v0.00\n", text)
        self.assertNotIn("  vehicle-0\n", text)
        self.assertIn("  summary cat: 2:0000000009-0000000002", text)

    def test_a_difference_below_the_rounding_leaves_the_lanes_text_alike(self):
        bob = JAMES.replace("pos=0 speed=0", "pos=0.001 speed=0")
        code, text = self.run_diff(*self.logs(james=JAMES, bob=bob))
        self.assertEqual(code, 1)
        self.assertIn("1 of 2 entries differ; the lane's text agrees", text)

    def test_incomplete_matching_summaries_do_not_claim_the_lane_text_agrees(self):
        bob = JAMES.replace("pos=0 speed=0", "pos=0.001 speed=0").replace(
            "3 entries written", "3 entries written and 1 left out"
        )
        code, text = self.run_diff(*self.logs(james=JAMES, bob=bob))
        self.assertEqual(code, 1)
        self.assertIn("captured summaries match; full lane state not established", text)
        self.assertNotIn("the lane's text agrees", text)
        self.assertIn("checkpoint reports 1 entry/entries left out", text)

    def test_an_entry_one_game_lacks_and_ignored_fields(self):
        bob = "\n".join(l for l in JAMES.splitlines() if "vehicle-0" not in l).replace(
            "entity=402", "entity=999").replace("3 entries written", "2 entries written")
        code, text = self.run_diff(*self.logs(james=JAMES, bob=bob))
        self.assertEqual(code, 1, "an observed field difference keeps exit status 1")
        self.assertIn("bob: not captured (incomplete dump)", text)
        self.assertIn("entry coverage incomplete", text)
        self.assertIn("entity=402", text)
        code, text = self.run_diff("--ignore", "entity", *self.logs(james=JAMES, bob=bob))
        self.assertEqual(code, 2, "without a known difference, incomplete coverage is inconclusive")
        self.assertNotIn("entity=", text)
        self.assertIn("captured entries agree", text)

    def test_repeated_steps_are_kept_and_paired_by_occurrence(self):
        again = JAMES + JAMES.replace("pos=0 speed=0", "pos=7 speed=0")
        code, text = self.run_diff(*self.logs(james=again, bob=again))
        self.assertEqual(code, 0, text)
        self.assertIn("step 300 occurrence 1", text)
        self.assertIn("step 300 occurrence 2", text)
        code, text = self.run_diff("--lane", "0", *self.logs(james=JAMES, bob=JAMES))
        self.assertEqual(code, 2)
        self.assertIn("no lane dumps", text)

    def test_a_missing_checkpoint_trailer_is_inconclusive(self):
        without_trailer = JAMES.replace(
            "[1759240010] lane dump at step 300: lanes 3, 3 entries written\n", ""
        )
        code, text = self.run_diff(*self.logs(james=without_trailer, bob=JAMES))
        self.assertEqual(code, 2)
        self.assertIn("checkpoint trailer missing", text)
        self.assertIn("captured summaries match; full lane state not established", text)

    def test_a_later_trailer_is_not_attached_to_an_earlier_same_step_capture(self):
        without_trailer = JAMES.replace(
            "[1759240010] lane dump at step 300: lanes 3, 3 entries written\n", ""
        )
        james = without_trailer + JAMES
        bob = JAMES + JAMES
        code, text = self.run_diff(*self.logs(james=james, bob=bob))
        self.assertEqual(code, 2)
        self.assertIn("step 300 occurrence 1", text)
        self.assertIn("step 300 occurrence 2", text)
        james_captures = lane_diff.read_dumps(james.splitlines())[(3, 300)]
        self.assertIn("checkpoint trailer missing", james_captures[0].issues())
        self.assertNotIn("checkpoint trailer missing", james_captures[1].issues())

    def test_interleaved_lanes_keep_open_captures_until_their_summary(self):
        interleaved = """\
lane 3 step 300 vehicle-0/path-0000 route_index=0 row=path-a
lane 0 step 300 row:edge-a p0=0,0,0 row=a
lane 3 step 300 vehicle-0 state=2 route_records=1 route_omitted=0 row=2
lane 0 step 300 summary 1:network
lane 3 step 300 summary 1:vehicle
lane dump at step 300: lanes 0,3, 5 entries written
"""
        code, text = self.run_diff(*self.logs(james=interleaved, bob=interleaved))
        self.assertEqual(code, 0, text)
        self.assertIn("lane 0 (network) step 300", text)
        self.assertIn("lane 3 (vehicles) step 300", text)

    def test_different_occurrence_counts_are_not_paired(self):
        again = JAMES + JAMES
        code, text = self.run_diff(*self.logs(james=again, bob=JAMES))
        self.assertEqual(code, 2)
        self.assertIn("occurrence alignment ambiguous", text)
        self.assertIn("captures not paired", text)

    def test_a_missing_summary_does_not_claim_lane_agreement(self):
        partial = JAMES.replace("[1759240010] lane 3 step 300 summary 2:0000000001-0000000002\n", "")
        partial = partial.replace("3 entries written", "2 entries written")
        code, text = self.run_diff(*self.logs(james=JAMES, bob=partial))
        self.assertEqual(code, 2)
        self.assertIn("the lane's text is not established", text)
        self.assertIn("missing summary", text)

    def test_missing_declared_route_records_make_the_capture_incomplete(self):
        complete = """\
lane 3 step 300 vehicle-0/path-0000 route_index=0 row=path-a
lane 3 step 300 vehicle-0/path-0001 route_index=1 row=path-b
lane 3 step 300 vehicle-0 state=2 route_records=2 route_omitted=0 row=2
lane 3 step 300 summary 1:abc
lane dump at step 300: lanes 3, 4 entries written
"""
        partial = complete.replace(
            "lane 3 step 300 vehicle-0/path-0001 route_index=1 row=path-b\n", ""
        ).replace("4 entries written", "3 entries written")
        code, text = self.run_diff(*self.logs(james=complete, bob=partial))
        self.assertEqual(code, 2)
        self.assertIn("vehicle-0 declares 2 route record(s), captured 1", text)
        self.assertIn("entry coverage incomplete", text)

    def test_route_omissions_in_vehicle_detail_chunks_make_the_capture_incomplete(self):
        dump = """\
lane 3 step 300 vehicle-0 state=2 row=2
lane 3 step 300 vehicle-0/detail-004 route_records=1 route_omitted=2
lane 3 step 300 vehicle-0/path-0000 route_index=0 row=path-a
lane 3 step 300 summary 1:abc
lane dump at step 300: lanes 3, 4 entries written
"""
        code, text = self.run_diff(*self.logs(james=dump, bob=dump))
        self.assertEqual(code, 2)
        self.assertIn("vehicle-0 declares 2 route record(s) omitted", text)
        self.assertIn("entry coverage incomplete", text)

    def test_vehicle_core_is_supplemental_and_does_not_add_a_hashed_row(self):
        complete = """\
lane 3 step 300 vehicle-0 state=2 pos=0.125 row=2
lane 3 step 300 vehicle-0/core state=2 pos=0.125 path_count=3 row=2
lane 3 step 300 summary 1:abc
lane dump at step 300: lanes 3, 3 entries written
"""
        code, text = self.run_diff(*self.logs(james=complete, bob=complete))
        self.assertEqual(code, 0, text)
        self.assertIn("all 2 entries agree", text)

    def test_retained_core_rows_compare_positions_when_verbose_rows_are_missing(self):
        complete = """\
lane 3 step 300 vehicle-0 state=2 pos=0.125 row=2
lane 3 step 300 vehicle-0/core state=2 pos=0.125 path_count=3 row=2
lane 3 step 300 summary 1:abc
lane dump at step 300: lanes 3, 3 entries written
"""
        truncated = """\
lane 3 step 300 vehicle-0/core state=2 pos=0.250 path_count=3 row=2
lane 3 step 300 summary 1:abc
lane dump at step 300: lanes 3, 2 entries written
"""
        code, text = self.run_diff(*self.logs(james=complete, bob=truncated))
        self.assertEqual(code, 1, text)
        self.assertIn("captured summaries match; full lane state not established", text)
        self.assertIn("vehicle-0/core", text)
        self.assertIn("pos=0.125", text)
        self.assertIn("pos=0.250", text)
        self.assertIn("summary declares 1 row(s), captured 0", text)

    def test_checkpoint_trailer_reports_omitted_entries(self):
        partial = JAMES.replace(
            "3 entries written", "3 entries written and 2 left out (at most 20000 a checkpoint)"
        )
        code, text = self.run_diff(*self.logs(james=JAMES, bob=partial))
        self.assertEqual(code, 2)
        self.assertIn("checkpoint reports 2 entry/entries left out", text)
        self.assertIn("entry coverage incomplete", text)

    def test_a_dump_only_one_game_wrote(self):
        code, text = self.run_diff(*self.logs(james=JAMES, bob="nothing here\n"))
        self.assertEqual(code, 2)
        self.assertIn("dumped by james only", text)
        self.assertIn("captures not paired", text)

    def test_trailer_exposes_wholly_missing_requested_lane(self):
        missing = JAMES.replace("lanes 3, 3 entries written", "lanes 3,4, 3 entries written")
        code, text = self.run_diff(*self.logs(james=missing, bob=missing))
        self.assertEqual(code, 2, text)
        self.assertIn("lane 4 (economy) step 300", text)
        self.assertIn("checkpoint requested lane but no rows captured", text)

    def test_trailer_without_any_rows_is_an_incomplete_capture(self):
        empty = "lane dump at step 300: lanes 3, 0 entries written\n"
        code, text = self.run_diff(*self.logs(james=empty, bob=empty))
        self.assertEqual(code, 2, text)
        self.assertIn("checkpoint requested lane but no rows captured", text)

    def test_no_logs_with_a_dump_is_inconclusive(self):
        code, text = self.run_diff(*self.logs(james="nothing here\n", bob="also nothing\n"))
        self.assertEqual(code, 2)
        self.assertIn("no lane dumps", text)


if __name__ == "__main__":
    unittest.main()
