"""Summarize paired float_encoding CSV files without discarding samples."""
import argparse
import csv
import math
from pathlib import Path
from statistics import median


def read_samples(path: Path) -> dict:
    groups: dict = {}
    with path.open(encoding="utf-8-sig", newline="") as source:
        for row in csv.DictReader(source):
            key = (row["fixture"], row["format"], int(row["threads"]), row["library"])
            if key[1] not in ("json", "ndjson") or key[2] <= 0 or key[3] not in ("nextjson", "serde"):
                raise ValueError(f"{path}: invalid measurement key: {key}")
            count, seconds = int(row["operations"]), float(row["seconds"])
            latency = float(row["ns_per_op"])
            if count <= 0 or not all(math.isfinite(v) and v > 0 for v in (seconds, latency)):
                raise ValueError(f"{path}: non-positive or non-finite measurement")
            if abs(latency - seconds * 1e9 / count) > 0.002:
                raise ValueError(f"{path}: ns_per_op disagrees with raw timer/count")
            samples = groups.setdefault(key, {})
            sample = int(row["sample"])
            if sample in samples:
                raise ValueError(f"{path}: duplicate sample for {key}")
            samples[sample] = latency / 1000
    if not groups:
        raise ValueError(f"{path}: no measurements")
    for key, samples in groups.items():
        if sorted(samples) != list(range(1, len(samples) + 1)):
            raise ValueError(f"{path}: missing samples for {key}")
    return groups


def summarize(before_path: Path, after_path: Path) -> str:
    before, after = read_samples(before_path), read_samples(after_path)
    if before.keys() != after.keys():
        raise ValueError("before/after measurement groups differ")
    lines = [
        "| Fixture | Format | Threads | Before µs/op | After µs/op (min–max) | Serde µs/op | Before/after | After/Serde |",
        "|---|---|---:|---:|---:|---:|---:|---:|",
    ]
    for key in sorted(after):
        if key[3] != "nextjson":
            continue
        reference = (*key[:3], "serde")
        if reference not in after or not (before[key].keys() == after[key].keys() == after[reference].keys()):
            raise ValueError(f"unpaired samples for {key}")
        old = median(before[key].values())
        new = median(after[key].values())
        serde = median(after[reference].values())
        low, high = min(after[key].values()), max(after[key].values())
        lines.append(f"| {key[0]} | {key[1]} | {key[2]} | {old:.2f} | {new:.2f} ({low:.2f}–{high:.2f}) | {serde:.2f} | {old/new:.2f} | {new/serde:.2f} |")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    arguments.output.write_text(summarize(arguments.before, arguments.after), encoding="utf-8")
