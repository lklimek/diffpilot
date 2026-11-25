import type { DashboardItem, DashboardStatusCounts } from "../../../api/provisional/dashboard";
import { itemJudgement } from "../../../api/provisional/dashboardJudgement";
import { buttonClass } from "../../button/button";
import { MenuButton, MenuItem } from "../../menu/MenuButton";

export type WorkStatus = "all" | keyof DashboardStatusCounts;

const LABELS: Record<WorkStatus, string> = {
  all: "All",
  outdated: "Outdated judgement",
  not_judged: "Not judged yet",
  not_live: "Not live-updated",
  excluded: "Excluded and snoozed",
};

const DESCRIPTIONS: Partial<Record<WorkStatus, string>> = {
  outdated: "Judgement inputs changed after the LLM judged these items.",
  not_judged: "New items are judged automatically, within the ranking limits.",
  not_live: "Repositories without the GitHub App update with the nightly scan and Refresh.",
};

export function matchesStatus(item: DashboardItem, status: WorkStatus): boolean {
  const judgement = itemJudgement(item);
  switch (status) {
    case "all":
      return true;
    case "outdated":
      return judgement.state === "outdated";
    case "not_judged":
      return (
        judgement.judged_at === null &&
        ["not_judged", "queued", "judging", "capped"].includes(judgement.state)
      );
    case "not_live":
      return Boolean(item.repo) && item.live === false;
    case "excluded":
      return false;
  }
}

export function getStatusCounts(items: DashboardItem[], excluded: number): DashboardStatusCounts {
  return {
    outdated: items.filter((item) => matchesStatus(item, "outdated")).length,
    not_judged: items.filter((item) => matchesStatus(item, "not_judged")).length,
    not_live: items.filter((item) => matchesStatus(item, "not_live")).length,
    excluded,
  };
}

export function StatusFilter({
  status,
  counts,
  total,
  excludedDescription,
  onChange,
}: {
  status: WorkStatus;
  counts: DashboardStatusCounts;
  total: number;
  excludedDescription: string;
  onChange: (status: WorkStatus) => void;
}) {
  const outdated = counts.outdated > 0 ? `${counts.outdated} outdated` : null;
  return (
    <MenuButton
      label={`Status: ${LABELS[status]}${outdated ? `. ${outdated}` : ""}`}
      trigger={
        <>
          Status: {LABELS[status]} {outdated && <span className="font-semibold text-med">↻ {outdated}</span>}{" "}
          <span aria-hidden>▾</span>
        </>
      }
      className={`${buttonClass("plain", true)} flex-wrap ${outdated ? "border-med-bd" : ""}`}
    >
      {(Object.keys(LABELS) as WorkStatus[]).map((choice) => (
        <MenuItem
          key={choice}
          label={`${LABELS[choice]} · ${choice === "all" ? total : counts[choice]}`}
          selection={{ kind: "radio", checked: choice === status }}
          description={choice === "excluded" ? excludedDescription : DESCRIPTIONS[choice]}
          onSelect={() => onChange(choice)}
        />
      ))}
    </MenuButton>
  );
}
