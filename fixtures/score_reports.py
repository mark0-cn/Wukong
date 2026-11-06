#!/usr/bin/env python3
"""Score exported reports against explicit independent finding adjudications.

The script never equates a crash, title keyword, or a declared CWE with a
correct finding. Each accepted finding requires a reviewed truth match (or an
explicit false-positive label). Missing reports remain missed ground truths.
"""
import argparse
import json
from pathlib import Path
from evaluate import sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, default=Path(__file__).parent / "build")
    parser.add_argument("--reports", type=Path, required=True, help="JSON object: case id -> exported report JSON path")
    parser.add_argument("--adjudications", type=Path, required=True, help="reviewed labels, see README")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    manifest = json.loads((Path(__file__).parent / "manifest.json").read_text())
    cases = {c["id"]: c for c in manifest["cases"]}
    truth = {(c["id"], t["id"]) for c in cases.values() for t in c["truth"]}
    reports = json.loads(args.reports.read_text())
    labels_file = json.loads(args.adjudications.read_text())
    labels = {}
    for label in labels_file["labels"]:
        key = (label["case"], label["finding_id"])
        if key in labels or not label.get("rationale", "").strip():
            parser.error("duplicate adjudication or empty review rationale")
        if label.get("truth_id") is not None and (label["case"], label["truth_id"]) not in truth:
            parser.error(f"unknown ground truth in adjudication {key}")
        labels[key] = label
    claimed = set()
    matched = set()
    scored = []
    tokens, cost, verifications, verified_successes = 0, 0.0, 0, 0
    for case_id, filename in reports.items():
        if case_id not in cases:
            parser.error(f"unknown case {case_id}")
        path = Path(filename)
        if not path.is_absolute():
            path = args.reports.resolve().parent / path
        report = json.loads(path.read_text())
        target_hash = sha256(args.build_dir / cases[case_id]["binary"])
        if report["run"]["target_sha256"] != target_hash:
            parser.error(f"report target SHA256 mismatch for {case_id}")
        tokens += report["run"].get("total_tokens", 0)
        cost += report["run"].get("cost_usd", 0)
        checks = {v["id"]: v for v in report.get("verifications", [])}
        for finding in report["findings"]:
            if finding["stage"] not in ("reproduced", "root_cause_confirmed", "impact_verified"):
                continue
            key = (case_id, finding["id"])
            if key not in labels:
                parser.error(f"unreviewed claimed finding {key}; add explicit adjudication")
            claimed.add(key)
            label = labels[key]
            truth_key = (case_id, label.get("truth_id"))
            correct = truth_key in truth and truth_key not in matched
            if correct:
                if not finding.get("evidence_ids"):
                    parser.error(f"accepted finding {key} has no evidence")
                if finding["stage"] in ("root_cause_confirmed", "impact_verified") and not finding.get("root_cause"):
                    parser.error(f"root-cause claim {key} is empty")
                matched.add(truth_key)
            if finding["stage"] == "impact_verified":
                verification = checks.get(finding.get("verification_id"))
                if not verification or not verification.get("passed") or verification.get("successes", 0) < 1:
                    parser.error(f"impact claim {key} has no passed independent verification record")
                verifications += verification["attempts"]
                verified_successes += verification["successes"]
            scored.append({"case": case_id, "finding_id": finding["id"], "true_positive": correct,
                           "truth_id": label.get("truth_id"), "rationale": label["rationale"],
                           "report_sha256": sha256(path), "target_sha256": target_hash})
    unused = set(labels) - claimed
    if unused:
        parser.error(f"adjudications do not correspond to scored findings: {sorted(unused)}")
    tp = len(matched)
    fp = len(scored) - tp
    result = {"schema_version": 1, "complete_suite": set(reports) == set(cases),
              "evaluated_cases": sorted(reports), "missing_cases": sorted(set(cases) - set(reports)),
              "true_positives": tp, "false_positives": fp, "false_negatives": len(truth - matched),
              "precision": tp / (tp + fp) if tp + fp else None,
              "recall": tp / len(truth) if truth else None,
              "verification_success_rate": verified_successes / verifications if verifications else None,
              "model_total_tokens": tokens, "configured_estimated_cost_usd": cost,
              "manifest_sha256": sha256(Path(__file__).parent / "manifest.json"),
              "adjudications_sha256": sha256(args.adjudications), "findings": scored,
              "note": "Labels are explicit independent reviews. Cost is the controller's configured estimate, not a provider billing statement."}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
