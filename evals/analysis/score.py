#!/usr/bin/env python3
"""Score general-analysis answers using explicit independent question reviews.

No vulnerability finding or title/keyword match is required. Correct verdicts
must cite evidence IDs from the actual exported report and an independent
review rationale. Unrun tasks count as unresolved, never as correct.
"""
import argparse
import hashlib
import json
from pathlib import Path
from evals.paths import ANALYSIS


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def score(tasks, build, reports, labels):
    cases = {case["id"]: case for case in tasks["cases"]}
    questions = {(case["id"], question) for case in cases.values() for question in case["questions"]}
    if set(reports) - set(cases):
        raise ValueError("reports contain an unknown case")
    evidence = {}
    usage = {}
    for case_id, report in reports.items():
        snapshot = report["snapshot"]
        run = snapshot["run"]
        target = build["targets"][cases[case_id]["binary"]]
        if sha256(Path(target["path"])) != target["sha256"] or run["target_sha256"] != target["sha256"]:
            raise ValueError(f"target SHA256 mismatch for {case_id}")
        evidence[case_id] = {item["id"] for item in snapshot["evidence"]}
        values = (run.get("total_tokens", 0), run.get("cost_usd", 0.0))
        if run["id"] in usage and usage[run["id"]] != values:
            raise ValueError("inconsistent usage snapshots for the same run")
        usage[run["id"]] = values
    reviewed = {}
    for label in labels:
        key = (label["case"], label["question_id"])
        if key not in questions or key[0] not in reports or key in reviewed:
            raise ValueError(f"unknown, duplicate or unrun question review: {key}")
        if label["verdict"] not in {"correct", "incorrect", "unresolved"}:
            raise ValueError(f"invalid verdict for {key}")
        if not label.get("rationale", "").strip():
            raise ValueError(f"independent review rationale required for {key}")
        links = label.get("evidence_ids", [])
        if not isinstance(links, list) or not all(isinstance(item, str) for item in links):
            raise ValueError(f"invalid evidence list for {key}")
        if set(links) - evidence[key[0]] or (label["verdict"] == "correct" and not links):
            raise ValueError(f"correct answers require linked report evidence for {key}")
        reviewed[key] = label
    expected_reviews = {key for key in questions if key[0] in reports}
    if set(reviewed) != expected_reviews:
        raise ValueError(f"missing independent question reviews: {sorted(expected_reviews - set(reviewed))}")
    correct = sum(label["verdict"] == "correct" for label in reviewed.values())
    incorrect = sum(label["verdict"] == "incorrect" for label in reviewed.values())
    return {
        "schema_version": 1, "complete_suite": set(reports) == set(cases),
        "evaluated_cases": sorted(reports), "missing_cases": sorted(set(cases) - set(reports)),
        "total_questions": len(questions), "correct": correct, "incorrect": incorrect,
        "unresolved": len(questions) - correct - incorrect,
        "evidence_backed_correct_rate": correct / len(questions) if questions else None,
        "model_total_tokens": sum(values[0] for values in usage.values()),
        "configured_estimated_cost_usd": sum(values[1] for values in usage.values()),
        "reviews": list(reviewed.values()),
        "note": "Correctness labels are independent reviews, not inferred from crashes or tool success."
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, required=True)
    parser.add_argument("--tasks", type=Path, default=ANALYSIS / "tasks.json", help="source-controlled task manifest used by this fixture build")
    parser.add_argument("--reports", type=Path, required=True, help="JSON object: task case ID -> exported report.json")
    parser.add_argument("--adjudications", type=Path, required=True, help="JSON object with independently reviewed labels")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    tasks_path = args.tasks.resolve()
    build = json.loads((args.build_dir / "build.json").read_text())
    if build["tasks_sha256"] != sha256(tasks_path):
        parser.error("task manifest changed since the fixture build")
    paths = json.loads(args.reports.read_text())
    reports, hashes = {}, {}
    for case_id, filename in paths.items():
        path = Path(filename)
        if not path.is_absolute():
            path = args.reports.resolve().parent / path
        reports[case_id] = json.loads(path.read_text())
        hashes[case_id] = sha256(path)
    try:
        result = score(json.loads(tasks_path.read_text()), build, reports,
                       json.loads(args.adjudications.read_text())["labels"])
    except (ValueError, KeyError) as error:
        parser.error(str(error))
    result["report_sha256"] = hashes
    result["adjudications_sha256"] = sha256(args.adjudications)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
