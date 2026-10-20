#!/usr/bin/env python3
"""Diff lane dumps in two or more games' hook.logs (docs/HOOKS.md, "Lane dumps").

After the room says a game diverged, every game in the room writes the
diverged lanes to its hook.log at the next two checkpoints, one line per
entry. This tool pairs captures by lane, step, and occurrence, then compares
the entries by key. Incomplete diagnostic rows are reported as unknown rather
than as absent entities. The lane summary is the hash text the hook compared.

    python tools/lane_diff.py james=hook.log bob=bob/hook.log cat=cat/hook.log
    python tools/lane_diff.py --lane 3 --step 300 --max 20 a.log b.log
    python tools/lane_diff.py --ignore entity a.log b.log

A log is NAME=PATH or a PATH (named after its folder). Exits 1 when captured
entries differ, 2 when selected captures are incomplete or cannot be aligned
(including when there are no selected dumps), and 0 only when complete aligned
captures show no differences. Standard library only.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections import OrderedDict, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

LANES = {0: "network", 1: "constructions", 2: "lines", 3: "vehicles", 4: "economy", 5: "towns", 6: "people"}

# `[secs] lane <n> step <step> <key> <rest>`; the timestamp is optional.
ENTRY = re.compile(r"^(?:\[(\d+)\] )?lane (\d+) step (\d+) (\S+)(?: (.*))?$")
TRAILER = re.compile(
    r"^(?:\[\d+\] )?lane dump at step (\d+): lanes ([\d,]+), (\d+) entries written"
    r"(?: and (\d+) left out\b.*)?$"
)
SUMMARY_ROWS = re.compile(r"^(\d+):")
ROUTE_PATH = re.compile(r"^(vehicle-\d+)/path-\d+$")
VEHICLE_DETAIL = re.compile(r"^(vehicle-\d+)/detail-\d+$")
DETAIL_RECORD = re.compile(r"/(?:detail|path)-\d+$|/core$")


@dataclass
class Dump:
    lane: int
    step: int
    entries: list[tuple[str, str]] = field(default_factory=list)
    summary: str | None = None
    error: str | None = None
    closed: bool = False
    batch_issues: list[str] = field(default_factory=list)

    def issues(self) -> list[str]:
        """Known reasons that this capture may omit diagnostic entries."""
        out = list(self.batch_issues)
        if self.error is not None:
            out.append(f"lane read failed ({self.error})")
        elif self.summary is None:
            out.append("missing summary")
        elif not self.closed:
            out.append("capture did not end with a summary")

        if self.summary is not None and self.lane in {0, 1, 2, 3, 5}:
            count = SUMMARY_ROWS.match(self.summary)
            # These readers return `summary(rows)`. The economy summary is a
            # player id and balance (`player_id:balance...`), not a row count.
            if count:
                declared = int(count[1])
                captured = sum(
                    1 for key, _ in self.entries
                    if key not in ("summary", "err") and not DETAIL_RECORD.search(key)
                )
                if declared != captured:
                    out.append(f"summary declares {declared} row(s), captured {captured}")
        elif self.summary is not None and self.lane == 6:
            captured = sum(1 for key, _ in self.entries if key == "people")
            if captured != 1:
                out.append(f"people summary has {captured} aggregate entry/entries, expected 1")

        roots: dict[str, dict[str, str]] = {}
        route_counts: dict[str, int] = defaultdict(int)
        for key, rest in self.entries:
            match = ROUTE_PATH.match(key)
            if match:
                route_counts[match[1]] += 1
                continue
            detail = VEHICLE_DETAIL.match(key)
            if detail:
                roots.setdefault(detail[1], {}).update(fields(rest))
            elif key.startswith("vehicle-") and "/" not in key:
                roots.setdefault(key, {}).update(fields(rest))
        for key, row_fields in roots.items():
            expected = row_fields.get("route_records")
            if expected is not None and expected.isdigit():
                actual = route_counts.get(key, 0)
                if int(expected) != actual:
                    out.append(f"{key} declares {expected} route record(s), captured {actual}")
            omitted = row_fields.get("route_omitted")
            if omitted is not None and omitted.isdigit() and int(omitted) > 0:
                out.append(f"{key} declares {omitted} route record(s) omitted")
        return list(dict.fromkeys(out))

    def complete(self) -> bool:
        return not self.issues()


@dataclass
class CheckpointBatch:
    step: int
    lanes: set[int] = field(default_factory=set)
    dumps: list[Dump] = field(default_factory=list)


def read_dumps(lines):
    """Return ordered captures by `(lane, step)`, preserving each occurrence.

    Each `(lane, step)` capture stays open through entries from other lanes;
    summaries and err lines close their own lane. Checkpoint trailers validate
    the entries written in that batch and expose omissions. A batch without a
    trailer is incomplete and is kept separate from a later repeated step.
    """
    dumps: dict[tuple[int, int], list[Dump]] = OrderedDict()
    open_captures: dict[tuple[int, int], Dump] = {}
    batch: CheckpointBatch | None = None

    def finish_without_trailer():
        nonlocal batch
        if batch is None:
            return
        for dump in batch.dumps:
            dump.batch_issues.append("checkpoint trailer missing")
            open_captures.pop((dump.lane, dump.step), None)
        batch = None

    for line in lines:
        line = line.rstrip("\r\n")
        match = ENTRY.match(line)
        if match:
            _timestamp, lane_text, step_text, key, rest = match.groups()
            lane, step = int(lane_text), int(step_text)
            at = (lane, step)
            current = open_captures.get(at)
            if current is None:
                if batch is not None and (batch.step != step or lane in batch.lanes):
                    finish_without_trailer()
                if batch is None:
                    batch = CheckpointBatch(step)
                batch.lanes.add(lane)
                current = Dump(lane, step)
                dumps.setdefault(at, []).append(current)
                batch.dumps.append(current)
                open_captures[at] = current
            rest = rest or ""
            current.entries.append((key, rest))
            if key == "summary":
                current.summary = rest
                current.closed = True
                open_captures.pop(at, None)
            elif key == "err":
                current.error = rest
                current.closed = True
                open_captures.pop(at, None)
            continue

        trailer = TRAILER.match(line)
        if trailer:
            step, lane_text, written, left_out = trailer.groups()
            step, written = int(step), int(written)
            omitted = int(left_out or 0)
            if batch is not None and batch.step != step:
                finish_without_trailer()
            if batch is None:
                batch = CheckpointBatch(step)
            captured_lines = sum(len(dump.entries) for dump in batch.dumps)
            for lane in (int(part) for part in lane_text.split(",")):
                if lane not in batch.lanes:
                    missing = Dump(lane, step)
                    missing.batch_issues.append("checkpoint requested lane but no rows captured")
                    dumps.setdefault((lane, step), []).append(missing)
                    batch.dumps.append(missing)
            for dump in batch.dumps:
                if written != captured_lines:
                    dump.batch_issues.append(
                        f"checkpoint reports {written} entries written, captured {captured_lines}"
                    )
                if omitted:
                    dump.batch_issues.append(f"checkpoint reports {omitted} entry/entries left out")
                open_captures.pop((dump.lane, dump.step), None)
            batch = None

    finish_without_trailer()

    return dumps


def fields(rest: str) -> "OrderedDict[str, str]":
    """An entry's fields: `k=v` words, then `row=` to the end of the line."""
    out: OrderedDict[str, str] = OrderedDict()
    head, sep, row = rest.partition(" row=")
    if not sep and rest.startswith("row="):
        head, row, sep = "", rest[4:], "row="
    for word in head.split():
        name, eq, value = word.partition("=")
        out[name if eq else word] = value if eq else ""
    if sep:
        out["row"] = row
    return out


def strip(rest: str, ignore: set[str]) -> str:
    if not ignore:
        return rest
    return " ".join(f"{k}={v}" if v or k == "row" else k for k, v in fields(rest).items() if k not in ignore)


def keyed(entries):
    """`{key: [rest, ...]}`; repeated keys remain repeated entries."""
    out: dict[str, list[str]] = OrderedDict()
    for key, rest in entries:
        if key not in ("summary", "err"):
            out.setdefault(key, []).append(rest)
    return out


def summary_verdict(dumps_by_game: dict[str, Dump]) -> tuple[str, bool, bool]:
    unavailable = [
        f"{game}: {'lane read failed' if dump.error else 'missing summary'}"
        for game, dump in dumps_by_game.items()
        if dump.summary is None or dump.error is not None
    ]
    if unavailable:
        return "the lane's text is not established (" + "; ".join(unavailable) + ")", False, False
    summaries = {dump.summary for dump in dumps_by_game.values()}
    differs = len(summaries) != 1
    if any(not dump.complete() for dump in dumps_by_game.values()):
        label = "captured summaries differ; full lane state not established" if differs else "captured summaries match; full lane state not established"
    else:
        label = "the lane's text differs" if differs else "the lane's text agrees"
    return label, True, differs


def coverage_note(dumps_by_game: dict[str, Dump]) -> str:
    incomplete = [
        f"{game}: {', '.join(dump.issues())}"
        for game, dump in dumps_by_game.items()
        if not dump.complete()
    ]
    return "; entry coverage incomplete (" + "; ".join(incomplete) + ")" if incomplete else ""


def compare(dumps_by_game, lane, step, ignore, most, out, occurrence=None):
    """Compare one aligned capture occurrence; returns observed differing keys."""
    by_game = {game: keyed(dump.entries) for game, dump in dumps_by_game.items()}
    games = list(dumps_by_game)
    keys: list[str] = []
    seen = set()
    for game in games:
        for key in by_game[game]:
            if key not in seen:
                seen.add(key)
                keys.append(key)

    name = LANES.get(lane, "?")
    qualifier = f" occurrence {occurrence}" if occurrence is not None else ""
    head = f"lane {lane} ({name}) step {step}{qualifier}: {', '.join(games)}"
    differences = []
    uncertain = []
    for key in keys:
        values = {game: tuple(strip(r, ignore) for r in by_game[game].get(key, [])) for game in games}
        present = [value for value in values.values() if value]
        known_difference = len(set(present)) > 1
        missing = [game for game, value in values.items() if not value]
        if missing and any(dumps_by_game[game].complete() for game in missing) and present:
            known_difference = True
        if known_difference:
            differences.append((key, values))
        elif missing and present:
            uncertain.append((key, values))

    summary, _summary_available, summary_differs = summary_verdict(dumps_by_game)
    coverage = coverage_note(dumps_by_game)
    if not differences:
        if all(dump.complete() for dump in dumps_by_game.values()):
            out.write(f"{head}: all {len(keys)} entries agree; {summary}{coverage}\n")
        else:
            out.write(f"{head}: captured entries agree; {summary}{coverage}\n")
        if not uncertain:
            if summary_differs:
                for game, dump in dumps_by_game.items():
                    if dump.summary is not None:
                        out.write(f"  summary {game}: {dump.summary}\n")
            return int(summary_differs)
    else:
        if all(dump.complete() for dump in dumps_by_game.values()):
            result = f"{len(differences)} of {len(keys)} entries differ"
        else:
            result = f"{len(differences)} observed entry key(s) differ"
        out.write(f"{head}: {result}; {summary}{coverage}\n")

    def show_entry(key, values, uncertainty=False):
        out.write(f"  {key}{' (not comparable in every game)' if uncertainty else ''}\n")
        groups: dict[tuple[str, ...] | None, list[str]] = defaultdict(list)
        for game in games:
            groups[values[game] if values[game] else None].append(game)
        parsed = {value: [fields(r) for r in value] for value in groups if value is not None}
        firsts = [parts[0] if parts else OrderedDict() for parts in parsed.values()]
        names: list[str] = []
        for parsed_fields in firsts:
            for field_name in parsed_fields:
                if field_name not in names:
                    names.append(field_name)
        changed = [field_name for field_name in names if len({parsed_fields.get(field_name) for parsed_fields in firsts}) > 1]
        for value, members in groups.items():
            who = ",".join(members)
            if value is None:
                statuses = []
                for game in members:
                    if dumps_by_game[game].complete():
                        statuses.append(f"{game}: no such entry")
                    else:
                        statuses.append(f"{game}: not captured (incomplete dump)")
                out.write(f"    {', '.join(statuses)}\n")
                continue
            if len(value) > 1:
                out.write(f"    {who}: {len(value)} entries with this key\n")
            first = parsed[value][0]
            shown = " ".join(f"{field_name}={first[field_name]}" for field_name in changed if field_name in first) or value[0]
            out.write(f"    {who}: {shown}\n")

    for key, values in differences[:most]:
        show_entry(key, values)
    rest = len(differences) - most
    if rest > 0:
        out.write(f"  ... and {rest} more (--max)\n")
    for key, values in uncertain[:most]:
        show_entry(key, values, uncertainty=True)
    uncertain_rest = len(uncertain) - most
    if uncertain_rest > 0:
        out.write(f"  ... and {uncertain_rest} more entries with unknown coverage (--max)\n")
    if summary_differs:
        for game, dump in dumps_by_game.items():
            if dump.summary is not None:
                out.write(f"  summary {game}: {dump.summary}\n")
    return len(differences) or int(summary_differs)


def name_of(arg: str, taken: set[str]) -> tuple[str, Path]:
    if "=" in arg and not Path(arg).exists():
        name, path = arg.split("=", 1)
        return name, Path(path)
    path = Path(arg)
    name = path.parent.name or path.stem
    while name in taken:
        name += "'"
    return name, path


def main(argv=None, out=sys.stdout) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("logs", nargs="+", help="NAME=PATH or PATH of a hook.log")
    parser.add_argument("--lane", type=int, action="append", help="only this lane (repeatable)")
    parser.add_argument("--step", type=int, action="append", help="only this step (repeatable)")
    parser.add_argument("--max", type=int, default=10, help="entries shown per capture (10)")
    parser.add_argument("--ignore", action="append", default=[], help="a field not to compare, such as entity")
    args = parser.parse_args(argv)
    if len(args.logs) < 2:
        parser.error("give two or more logs")
    dumps_by_game: dict[str, dict] = OrderedDict()
    for arg in args.logs:
        name, path = name_of(arg, set(dumps_by_game))
        with open(path, encoding="utf-8", errors="replace") as f:
            dumps_by_game[name] = read_dumps(f)
    points = sorted({at for dumps in dumps_by_game.values() for at in dumps}, key=lambda at: (at[1], at[0]))
    if args.lane:
        points = [p for p in points if p[0] in args.lane]
    if args.step:
        points = [p for p in points if p[1] in args.step]
    if not points:
        out.write("no lane dumps in these logs (lines 'lane <n> step <step> ...')\n")
        return 2

    differ = 0
    inconclusive = False
    for lane, step in points:
        counts = {game: len(dumps.get((lane, step), [])) for game, dumps in dumps_by_game.items()}
        if len(set(counts.values())) != 1:
            inconclusive = True
            count_text = ", ".join(f"{game}={count}" for game, count in counts.items())
            present = [game for game, count in counts.items() if count]
            if len(present) == 1:
                note = f"dumped by {present[0]} only"
            else:
                note = f"occurrence alignment ambiguous (capture counts: {count_text})"
            out.write(
                f"lane {lane} ({LANES.get(lane, '?')}) step {step}: {note}; captures not paired\n"
            )
            for game, count in counts.items():
                for occurrence, dump in enumerate(dumps_by_game[game].get((lane, step), []), 1):
                    status = f"summary={dump.summary}" if dump.summary is not None else "summary missing"
                    issues = dump.issues()
                    if issues:
                        status += "; incomplete: " + ", ".join(issues)
                    out.write(f"  {game} occurrence {occurrence}: {status}\n")
            continue

        captures = counts[next(iter(counts))]
        for index in range(captures):
            occurrence = index + 1 if captures > 1 else None
            aligned = {
                game: dumps_by_game[game][(lane, step)][index]
                for game in dumps_by_game
            }
            if any(not dump.complete() for dump in aligned.values()):
                inconclusive = True
            differ += compare(aligned, lane, step, set(args.ignore), args.max, out, occurrence)
    if differ:
        return 1
    return 2 if inconclusive else 0


if __name__ == "__main__":
    sys.exit(main())
