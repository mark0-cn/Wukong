"""Compare paired, independently adjudicated agent/skill experiments; never infer truth from tool success."""
import argparse
from collections import defaultdict
from datetime import datetime
import json
from pathlib import Path
import statistics


def metrics(report, reviews):
    snapshot = report["snapshot"]
    evidence = {item["id"] for item in snapshot["evidence"]}
    seen = set()
    for review in reviews:
        if review["question_id"] in seen: raise ValueError("duplicate question review")
        seen.add(review["question_id"])
        if review.get("injected_false_claim") is True and not isinstance(review.get("accepted_by_agent"), bool): raise ValueError("false claim requires independent acceptance label")
        if review["verdict"] not in {"correct", "incorrect", "unresolved"}: raise ValueError("invalid verdict")
        if not review.get("rationale", "").strip(): raise ValueError("independent rationale required")
        if set(review.get("evidence_ids", [])) - evidence: raise ValueError("unknown review evidence")
        if review["verdict"] == "correct" and not review.get("evidence_ids"): raise ValueError("correct requires evidence")
    if not reviews: raise ValueError("reviews required")
    calls = snapshot.get("calls", [])
    # This is a duplicate-read proxy, not a claim that all repeated analysis is waste.
    reads = [json.dumps([c["name"], c["arguments"]], sort_keys=True) for c in calls if not c["side_effecting"]]
    run = snapshot["run"]
    elapsed = None
    if run.get("status") in {"completed", "failed", "crashed", "cancelled"} and run.get("updated_at") and run.get("created_at"):
        elapsed = (datetime.fromisoformat(run["updated_at"].replace("Z", "+00:00")) - datetime.fromisoformat(run["created_at"].replace("Z", "+00:00"))).total_seconds()
    false_reviews = [r for r in reviews if r.get("injected_false_claim") is True]
    return {"accuracy":sum(r["verdict"] == "correct" for r in reviews)/len(reviews),
        "incorrect_rate":sum(r["verdict"] == "incorrect" for r in reviews)/len(reviews),
        "unresolved_rate":sum(r["verdict"] == "unresolved" for r in reviews)/len(reviews),
        "tokens":run["total_tokens"],"cost_usd":run["cost_usd"],"elapsed_seconds":elapsed,
        "tool_calls":len(calls),"failed_calls":sum(c["state"] == "failed" for c in calls),
        "duplicate_read_proxy":len(reads)-len(set(reads)),
        "false_claim_acceptance_rate":sum(r.get("accepted_by_agent") is True for r in false_reviews)/len(false_reviews) if false_reviews else None,
        "verification_records":len(snapshot.get("analysis_verifications", [])),
        "agents":len(snapshot.get("agents", []))}


def compare(experiments):
    pairs = defaultdict(dict)
    for experiment in experiments:
        key = (experiment["case"], experiment["repeat"])
        condition = experiment["condition"]
        if condition not in {"baseline", "candidate"}: raise ValueError("invalid condition")
        if condition in pairs[key]: raise ValueError("duplicate paired run")
        pairs[key][condition] = experiment
    if not pairs: raise ValueError("no experiments")
    deltas = []
    for key, pair in sorted(pairs.items()):
        if set(pair) != {"baseline", "candidate"}: raise ValueError("unpaired experiment")
        left, right = pair["baseline"], pair["candidate"]
        for field in ["model", "budget", "target_sha256", "questions"]:
            if left[field] != right[field]: raise ValueError("confounded pair: " + field)
        for entry in [left, right]:
            if entry["report"]["snapshot"]["run"]["target_sha256"] != entry["target_sha256"]: raise ValueError("target mismatch")
            if set(entry["questions"]) != {r["question_id"] for r in entry["reviews"]}: raise ValueError("missing question reviews")
        a, b = metrics(left["report"], left["reviews"]), metrics(right["report"], right["reviews"])
        deltas.append({"case":key[0],"repeat":key[1],"baseline":a,"candidate":b,
            "delta":{name:b[name]-a[name] if a[name] is not None and b[name] is not None else None for name in a}})
    means = {name:statistics.mean(values) if values else None for name in deltas[0]["delta"]
        for values in [[row["delta"][name] for row in deltas if row["delta"][name] is not None]]}
    return {"pairs":deltas,"mean_paired_delta":means,
        "note":"Candidate minus baseline. Requires independent adjudication. Duplicate reads are only a proxy; missing durations remain null."}

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    experiments = json.loads(args.manifest.read_text(encoding="utf-8"))
    for entry in experiments:
        for field in ["report", "reviews"]:
            entry[field] = json.loads((args.manifest.parent / entry[field]).read_text(encoding="utf-8"))
    args.output.write_text(json.dumps(compare(experiments), indent=2), encoding="utf-8")
