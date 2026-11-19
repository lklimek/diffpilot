"""Synthetic golden for `golden.rs`: seeded invented items, expected ranking from the skill's own Python scoring.

Input: invented repositories, pull requests, issues and TODOs from a seeded generator, spread over every rule
branch and bucket boundary of the scoring (no data from any real run). Expected output: the daily-priorities
skill's reference (`scoring.pr_eligibility`, `pr_score_parts`, `issue_score_parts`, `todos.score_row`,
`score.merge`), imported read-only from SKILL_DIR (no bytecode written, no network).

    python3 -I make_golden.py SKILL_DIR/scripts > golden-run.json
"""

import datetime as dt
import json
import random
import sys
import uuid
from collections import Counter

SKILL_DIR = sys.argv[1]
sys.dont_write_bytecode = True
sys.path.insert(0, SKILL_DIR)
import score  # noqa: E402
import scoring  # noqa: E402
import todos  # noqa: E402

SEED = 20261118
NOW = dt.datetime(2030, 3, 14, 9, 26, 53, 589_793, tzinfo=dt.timezone.utc)
USERS = ["alice", "alice-ci"]
# The skill hard-codes its one ownership-policy repository; the fixture names it `acme/core`.
POLICY_REPO = "dashpay/platform"
REPOS = {"acme/core": "CORE", "acme/wallet": "WAL", "acme/node": "NODE", "acme/docs": "DOC", "acme/sdk": "SDK"}
CONTEXTS = {
    "acme/core": {"releaseTags": ["v2.3.0", "v2.4.1", "v2.4.0"], "milestoneTitles": ["v2.4", "v2.5", "v3.0", "v2.3.x"]},
    "acme/wallet": {"releaseTags": ["1.0.0", "1.2.0+build.7"], "milestoneTitles": ["1.2", "1.3", "1.10", "2.0"]},
    "acme/node": {"releaseTags": ["v0.9.0", "nightly"], "milestoneTitles": ["v0.9", "v1.0"]},
    "acme/docs": {"releaseTags": None, "milestoneTitles": ["v1.1"]},
    "acme/sdk": {"releaseTags": ["v4.0.0"], "milestoneTitles": None},
}
PART_ORDER = scoring.PART_ORDER
# Ages in seconds around every bucket edge (2 d, 7 d waiting; 15 d recency zero) plus a clock-skew future time.
EDGES = [-60, 0, 3_600, 172_800, 172_801, 302_400, 604_800, 604_801, 1_296_000, 2_592_000, 31_536_000]
COVERAGE = Counter()


def ts(age_s):
    return (NOW - dt.timedelta(seconds=age_s)).isoformat().replace("+00:00", "Z")


def days(age_s):
    return None if age_s is None else age_s / 86400


def pick(rng, *weighted):
    values, weights = zip(*weighted)
    return rng.choices(values, weights)[0]


def age(rng):
    return rng.choice(EDGES) if rng.random() < 0.5 else rng.randint(0, 60 * 86400)


def maybe(rng, p, value):
    return value if rng.random() < p else None


def make_pr(rng, repo, number):
    is_draft = rng.random() < 0.15
    author = pick(
        rng, ("alice", 4), ("alice-ci", 1), ("bob", 4), ("carol", 2), ("thepastaclaw", 1), ("renovate[bot]", 1)
    )
    labels = rng.sample(["ready-for-human", "Blocked", "bug", "area: ui", "not-blocked"], rng.randint(0, 2))
    relations = sorted(rng.sample(["authored", "assigned", "review_requested"], rng.randint(1, 2)))
    base_default = pick(
        rng, (("main", "main"), 5), (("feat/x", "main"), 2), (("v1.0-dev", "v1.0-dev"), 1), (("main", None), 1)
    )
    milestone_kind = pick(rng, ("none", 4), ("version", 4), ("text", 1))
    titles = CONTEXTS[repo]["milestoneTitles"] or ["v9.9"]
    milestone = (
        None if milestone_kind == "none" else {"title": rng.choice(titles) if milestone_kind == "version" else "Next"}
    )
    requested = rng.random() < 0.4
    request_age = age(rng) if requested and rng.random() < 0.85 else None
    cr_human = rng.random() < 0.2
    comment_age = maybe(rng, 0.6, age(rng))
    pr = {
        "repo": repo,
        "number": number,
        "author": author,
        "authorIsBot": author.endswith("[bot]"),
        "relations": relations,
        "isDraft": is_draft,
        "labels": labels,
        "milestone": milestone,
        "milestoneContext": CONTEXTS[repo],
        "dependentPRs": maybe(rng, 0.9, sorted(rng.sample(range(1, 999), rng.choice([0, 0, 1, 2, 3, 4])))),
        "stackedOnPRs": maybe(rng, 0.9, sorted(rng.sample(range(1, 999), rng.choice([0, 0, 0, 1])))),
        "baseRefName": base_default[0],
        "defaultBranchName": base_default[1],
        "reviewPendingFor": ["alice"] if requested else [],
        "reviewRequestFreshnessDays": days(request_age),
        "reviewRequestFreshnessIsApprox": requested and rng.random() < 0.2,
        "reviewDecision": pick(rng, (None, 3), ("CHANGES_REQUESTED", 2), ("APPROVED", 1), ("REVIEW_REQUIRED", 2)),
        "changesRequestedBy": rng.choice([[], ["dave"], ["coderabbitai"]]),
        "changesRequestedByHuman": cr_human,
        "changesRequestedHumanAgeDays": days(age(rng)) if cr_human and rng.random() < 0.7 else None,
        "reviewsTruncated": rng.random() < 0.2,
        "awaitingOurReply": comment_age is not None and rng.random() < 0.6,
        "lastCommentAuthor": "erin" if comment_age is not None else None,
        "lastCommentAgeDays": days(comment_age),
        "staleDays": days(age(rng)),
        "additions": rng.choice([0, 3, 40, 400, 1200, 9000]) + rng.randint(0, 99),
        "deletions": rng.randint(0, 2000),
        "ciEffective": rng.choice(["green", "red", "pending", "unknown", "none"]),
        "mergeable": rng.choice(["MERGEABLE", "CONFLICTING", "UNKNOWN"]),
        "unresolvedThreadsTotal": rng.choice([0, 0, 1, 3]),
        "ownedChange": {"verdict": rng.choice(["none", "minor", "significant", "significant"])}
        if repo == "acme/core"
        else None,
        "body": rng.choice(["", "", "Depends on acme/node#12", "This is blocked by infra.", "unblocked by #3"]),
    }
    pr["_request_age"], pr["_cr_age"], pr["_comment_age"] = request_age, None, comment_age
    return pr


def eligibility(pr):
    """The skill's `pr_eligibility`, with diffpilot's one deviation (D19): the ownership policy never drops a
    pull request directed at the user (authored by a scanned login, assigned to one, or a direct review request)."""
    ok, reason = scoring.pr_eligibility(pr, USERS)
    directed = (
        (pr["author"] or "").lower() in USERS or "assigned" in pr["relations"] or bool(pr["reviewPendingFor"])
    )
    if not ok and "ownedChange" in reason and directed:
        return True, "directed at you (the ownership policy applies only to other pull requests)"
    return ok, reason


def make_issue(rng, repo, number):
    milestone = pick(rng, ("absent", 1), ("none", 4), ("version", 3))
    issue = {
        "repo": repo,
        "number": number,
        "author": rng.choice(["bob", "alice", "frank"]),
        "relations": sorted(rng.sample(["repo_open", "assigned", "mentioned"], rng.randint(1, 2))),
        "labels": rng.sample(["Blocked", "bug", "help wanted"], rng.randint(0, 1)),
        "updatedDays": days(age(rng)),
        "milestoneContext": CONTEXTS[repo],
        "body": rng.choice(["", "crash on start", "Blocked\n on the release", "depends on #4"]),
    }
    if milestone != "absent":
        titles = CONTEXTS[repo]["milestoneTitles"] or ["v9.9"]
        issue["milestone"] = None if milestone == "none" else {"title": rng.choice(titles)}
    return issue


def allowed(name):
    values, (lo, hi) = scoring.COMPONENTS[name]
    return list(values) if values else list(range(int(lo), int(hi) + 1))


def iso_of(days_value):
    return None if days_value is None else ts(round(days_value * 86400))


def main():
    rng = random.Random(SEED)
    repos = list(REPOS)
    prs, issues, used = [], [], set()
    for _ in range(100):
        repo = rng.choice(repos)
        number = rng.randint(1, 3000)
        while (repo, number) in used:
            number += 1
        used.add((repo, number))
        prs.append(make_pr(rng, repo, number))
    for _ in range(30):
        repo = rng.choice(repos)
        number = rng.randint(1, 3000)
        while (repo, number) in used:
            number += 1
        used.add((repo, number))
        issues.append(make_issue(rng, repo, number))

    # Reference: scan items as the skill's scan.py emits them.
    scan = {"generated_at": NOW.isoformat(), "users": USERS, "prs": [], "issues": [], "errors": []}
    for pr in prs:
        ref = dict(pr, repo=POLICY_REPO if pr["repo"] == "acme/core" else pr["repo"])
        ok, reason = eligibility(ref)
        parts = scoring.pr_score_parts(ref, USERS, pr["body"])
        scan["prs"].append(
            {
                **pr,
                "id": f"{REPOS[pr['repo']]}-{pr['number']}",
                "kind": "pr",
                "title": "change",
                "url": "https://example.invalid/pr",
                "eligible": ok,
                "eligibilityReason": reason,
                "scoreParts": parts,
            }
        )
        COVERAGE[("eligibility", reason.split(" ")[0] if ok else reason.split("=")[-1].split(" ")[0])] += 1
    for issue in issues:
        parts = scoring.issue_score_parts(issue, issue["body"])
        scan["issues"].append(
            {
                **issue,
                "id": f"{REPOS[issue['repo']]}-{issue['number']}",
                "kind": "issue",
                "title": "report",
                "url": "https://example.invalid/issue",
                "eligible": True,
                "eligibilityReason": "open issue",
                "scoreParts": parts,
            }
        )

    todo_records = []
    for i in range(30):
        uid = str(uuid.UUID(int=rng.getrandbits(128), version=4))
        todo_records.append(
            {
                "id": uid,
                "project": "proj",
                "description": "",
                "title": rng.choice(["write docs", "fix panic in sync", "[HIGH] rotate keys", "tidy"]),
                "status": rng.choice(["pending", "pending", "in_progress", "blocked", "postponed"]),
                "priority": rng.choice(["high", "medium", "low", None]),
                "created_at": ts(rng.randint(0, 6000) * 864),
            }
        )
    prefixes = todos.prefixes([t["id"] for t in todo_records])
    judgment = {
        f"TODO-{prefixes[t['id']]}": todos.score_row(t, NOW, "https://example.invalid/ui") for t in todo_records
    }

    # Judgement: every open part answered, some defaults replaced, some computed parts overridden with a basis.
    items = scan["prs"] + scan["issues"]
    judged = {}  # the judged values only, as the Rust port receives them
    for item in items:
        if not item["eligible"] and rng.random() < 0.7:
            continue
        parts = {}
        for name, p in item["scoreParts"].items():
            if p["status"] == "needs_judgment":
                parts[name] = {"value": rng.choice(allowed(name)), "basis": "synthetic"}
            elif p["status"] == "provisional":
                value = p["value"] if rng.random() < 0.5 else rng.choice(allowed(name))
                parts[name] = {"value": value, "basis": "synthetic"}
            elif name == "unblocks" and rng.random() < 0.08:
                parts[name] = {"value": rng.choice(allowed(name)), "basis": "synthetic override"}
        judged[item["id"]] = {name: v["value"] for name, v in parts.items()}
        entry = {"parts": parts}
        # Deviation: an eligibility answer counts only while eligibility is asked (an unreadable ownership policy);
        # no synthetic item asks, so the drawn flip is not applied (the draw keeps the seeded sequence).
        rng.random()
        judgment[item["id"]] = entry
    for todo_id, row in judgment.items():
        if todo_id.startswith("TODO-"):
            judged[todo_id] = {}
            for name in (
                "defect_severity",
                "change_impact",
                "completion",
                "urgency_bonus",
                "unblocks",
                "blocked_penalty",
            ):
                if rng.random() < 0.4:
                    value = rng.choice(allowed(name))
                    row["parts"][name] = {"value": value, "basis": "synthetic"}
                    judged[todo_id][name] = value
    # Merges: two survivors each absorb one TODO and one scan item (absorbed rows carry no judged values).
    survivors = [i["id"] for i in items if i["id"] in judgment][:2]
    targets = [i["id"] for i in items if i["id"] in judgment][-2:]
    todo_targets = [k for k in judgment if k.startswith("TODO-")][:2]
    for owner, target, todo_target in zip(survivors, targets, todo_targets):
        judgment[owner]["alsoIds"] = [target, todo_target]
        judgment[target]["parts"] = {}
        judgment[todo_target]["parts"] = dict.fromkeys(judgment[todo_target]["parts"])
        judged[target], judged[todo_target] = {}, {}

    result = score.merge(scan, judgment)
    if result["unscored"]:
        raise SystemExit(f"unscored rows: {[r['id'] for r in result['unscored']]}")
    for row in result["ranked"]:
        for name in PART_ORDER:
            p = row["parts"][name]
            COVERAGE[(name, p["status"], round(p["value"], 1) if name not in ("recency", "change_size") else "x")] += 1

    out = fixture(scan, judgment, judged, result, prefixes, todo_records)
    sys.stdout.write(out)
    print(
        f"items {len(items) + len(todo_records)}, ranked {len(result['ranked'])}, "
        f"excluded {len(result['excluded'])}, distinct (component, status, value) branches "
        f"{sum(1 for k in COVERAGE if len(k) == 3)}, eligibility reasons {sum(1 for k in COVERAGE if k[0] == 'eligibility')}",
        file=sys.stderr,
    )


def fixture(scan, judgment, judged, result, prefixes, todo_records):
    key_of = {}
    prs, issues, todo_rows = [], [], []
    for pr in scan["prs"]:
        owner, name = pr["repo"].split("/")
        key_of[pr["id"]] = f"{owner}~{name}~pr~{pr['number']}"
        owned = (pr["ownedChange"] or {}).get("verdict")
        prs.append(
            {
                "repo": {"owner": owner, "name": name},
                "number": pr["number"],
                "title": "change",
                "body": pr["body"],
                "author": pr["author"],
                "author_is_bot": pr["authorIsBot"],
                "relations": pr["relations"],
                "is_draft": pr["isDraft"],
                "labels": pr["labels"],
                "updated_at": iso_of(pr["staleDays"]),
                "additions": pr["additions"],
                "deletions": pr["deletions"],
                "base_ref": pr["baseRefName"],
                "default_branch": pr["defaultBranchName"],
                "dependents": pr["dependentPRs"],
                "stacked_on": pr["stackedOnPRs"],
                "milestone": "unassigned" if pr["milestone"] is None else {"assigned": pr["milestone"]["title"]},
                "milestone_context": context(pr),
                "ci": pr["ciEffective"],
                "mergeable": pr["mergeable"].lower(),
                "review_decision": pr["reviewDecision"] and pr["reviewDecision"].lower(),
                "unresolved_threads": pr["unresolvedThreadsTotal"],
                "review_pending_for_us": bool(pr["reviewPendingFor"]),
                "review_requested_at": iso_of(pr["reviewRequestFreshnessDays"]),
                "review_request_age_is_approx": pr["reviewRequestFreshnessIsApprox"],
                "changes_requested_by": pr["changesRequestedBy"],
                "changes_requested_by_human": pr["changesRequestedByHuman"],
                "changes_requested_human_at": iso_of(pr["changesRequestedHumanAgeDays"]),
                "reviews_truncated": pr["reviewsTruncated"],
                "awaiting_our_reply": pr["awaitingOurReply"],
                "last_comment_author": pr["lastCommentAuthor"],
                "last_comment_at": iso_of(pr["lastCommentAgeDays"]),
                "owned_change": owned or "no_policy",
                "approved_by": [],
                "review_requested_from_others": [],
                "others_review_requested_at": None,
            }
        )
    for issue in scan["issues"]:
        owner, name = issue["repo"].split("/")
        key_of[issue["id"]] = f"{owner}~{name}~issue~{issue['number']}"
        if "milestone" not in issue:
            milestone = "unknown"
        else:
            milestone = "unassigned" if issue["milestone"] is None else {"assigned": issue["milestone"]["title"]}
        issues.append(
            {
                "repo": {"owner": owner, "name": name},
                "number": issue["number"],
                "title": "report",
                "body": issue["body"],
                "author": issue["author"],
                "relations": issue["relations"],
                "labels": issue["labels"],
                "updated_at": iso_of(issue["updatedDays"]),
                "milestone": milestone,
                "milestone_context": context(issue),
            }
        )
    for t in todo_records:
        key_of[f"TODO-{prefixes[t['id']]}"] = f"memcan~{t['id']}"
        todo_rows.append(
            {
                "id": t["id"],
                "title": t["title"],
                "status": t["status"],
                "priority": t["priority"],
                "changed_at": t["created_at"],
            }
        )
    judgements = {}
    for item_id, entry in judgment.items():
        points = [None] * len(PART_ORDER)
        for name, value in judged.get(item_id, {}).items():
            points[PART_ORDER.index(name)] = int(value)
        row = {"p": points}
        if "eligible" in entry:
            row["e"] = entry["eligible"]
        if entry.get("alsoIds"):
            row["a"] = [key_of[t] for t in entry["alsoIds"]]
        if any(v is not None for v in points) or len(row) > 1:
            judgements[key_of[item_id]] = row
    ranked = [
        {
            "id": r["id"],
            "tier": r["tier"].lower(),
            "score": r["score"],
            "tenths": [round(r["parts"][n]["value"] * 10) for n in PART_ORDER],
        }
        for r in result["ranked"]
    ]
    settings = {
        "linked_logins": ["alice-ci"],
        "repositories": [
            {
                "repo": {"owner": r.split("/")[0], "name": r.split("/")[1]},
                "alias": a,
                "watch_issues": False,
                "ownership_policy": None,
                "branch_patterns": [],
            }
            for r, a in REPOS.items()
        ],
        "enumerate_owners": [],
        "lookback_days": 7,
        "ready_label": "ready-for-human",
        "skip_authors": [],
        "include_todos": True,
        "memcan_extra_projects": [],
        "schedule_enabled": True,
        "schedule_time": "06:00",
        "time_zone": None,
    }
    doc = {
        "now": NOW.isoformat().replace("+00:00", "Z"),
        "own_login": "alice",
        "settings": settings,
        "prs": prs,
        "issues": issues,
        "todos": todo_rows,
        "judgements": judgements,
        "ranked": ranked,
        "excluded": [r["id"] for r in result["excluded"]],
    }
    lines = []
    for k, v in doc.items():
        if isinstance(v, list) and v and isinstance(v[0], dict):
            body = ",\n".join("  " + json.dumps(x, separators=(",", ":")) for x in v)
            lines.append(f'"{k}": [\n{body}\n]')
        elif k == "judgements":
            body = ",\n".join(f"  {json.dumps(a)}: {json.dumps(x, separators=(',', ':'))}" for a, x in v.items())
            lines.append(f'"{k}": {{\n{body}\n}}')
        else:
            lines.append(f'"{k}": {json.dumps(v, separators=(",", ":"))}')
    return "{\n" + ",\n".join(lines) + "\n}\n"


def context(item):
    ctx = item["milestoneContext"]
    return {"release_tags": ctx["releaseTags"], "milestone_titles": ctx["milestoneTitles"]}


if __name__ == "__main__":
    main()
