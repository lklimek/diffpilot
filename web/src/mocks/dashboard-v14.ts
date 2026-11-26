import type { WorkItem, DashboardSummary, Judgement } from "../api/generated/types";

const REPO_WITHOUT_APP = "docs-site";

export function withDashboardFacts(item: WorkItem, noApp: boolean): WorkItem {
  const review = item.tags.find((tag) => tag.text.startsWith("review requested"));
  const tags = item.tags.map((tag) =>
    tag === review ? { ...tag, filter: { key: "review_requested", label: "review requested" } } : tag,
  );
  const judgement: Judgement = {
    state: "current",
    judged_at: item.llm?.judged_at ?? null,
    outdated_because: null,
  };
  const info: WorkItem = { ...item, tags, judgement };
  if (item.repo) info.live = !noApp && item.repo.name !== REPO_WITHOUT_APP;
  if (item.id === "NC-490") info.watched = true;
  if (item.id === "PLAT-1203") {
    info.stack = { position: "bottom", size: 3, dependents: [1205, 1209] };
    info.release_base = "release/2.4";
    info.facts = [
      ...item.facts.filter(({ label }) => label !== "Base branch"),
      { label: "Base branch", value: "release/2.4" },
    ];
  }
  if (item.id === "NC-482" || item.id === "WUI-311") {
    judgement.state = "outdated";
    judgement.outdated_because = item.id === "NC-482" ? "facts_changed" : "prompt_changed";
    info.score = { ...item.score, state: "provisional" };
  }
  if (item.id === "CON-1534") {
    judgement.state = "judging";
    judgement.outdated_because = "facts_changed";
    info.score = { ...item.score, state: "provisional" };
  }
  if (item.id === "WUI-320" || item.id === "PLAT-1210" || item.id === "TODO-91bd0a44") {
    judgement.state = item.id === "WUI-320" ? "queued" : item.id === "PLAT-1210" ? "capped" : "not_judged";
    judgement.judged_at = null;
    info.llm = null;
    info.score = { ...item.score, state: "unjudged" };
  }
  return info;
}

export function dashboardCoverage(
  items: WorkItem[],
  excluded: number,
  lastEventAt: string | null,
): Pick<DashboardSummary, "live_repositories" | "not_live_repositories" | "last_event_at" | "status_counts"> {
  const repos = new Map(
    items.flatMap((item) =>
      item.repo ? [[`${item.repo.owner}/${item.repo.name}`, item.live] as const] : [],
    ),
  );
  return {
    live_repositories: [...repos.values()].filter((live) => live === true).length,
    not_live_repositories: [...repos.values()].filter((live) => live === false).length,
    last_event_at: lastEventAt ?? undefined,
    status_counts: {
      outdated: items.filter(
        ({ judgement }) => judgement?.state === "outdated" || judgement?.state === "judging",
      ).length,
      not_judged: items.filter(
        ({ judgement }) =>
          judgement?.state === "not_judged" || judgement?.state === "queued" || judgement?.state === "capped",
      ).length,
      not_live: items.filter(({ live }) => live === false).length,
      excluded,
    },
  };
}
