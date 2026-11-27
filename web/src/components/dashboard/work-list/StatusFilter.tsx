import type { WorkItem, StatusCounts } from "../../../api/generated/types";
import { buttonClass } from "../../button/button";
import { MenuButton, MenuItem } from "../../menu/MenuButton";

export type WorkStatus = "all" | keyof StatusCounts;

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

export function matchesStatus(item: WorkItem, status: WorkStatus): boolean {
  const state = item.judgement?.state;
  switch (status) {
    case "all":
      return true;
    case "outdated":
      return state === "outdated" || state === "judging";
    case "not_judged":
      return state === "not_judged" || state === "queued" || state === "capped";
    case "not_live":
      return Boolean(item.repo) && item.live === false;
    case "excluded":
      return false;
  }
}

export function StatusFilter({
  status,
  counts,
  total,
  excludedDescription,
  descriptions,
  onChange,
}: {
  status: WorkStatus;
  counts: StatusCounts;
  total: number;
  excludedDescription: string;
  descriptions?: Partial<Record<WorkStatus, string>>;
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
          description={
            choice === "excluded" ? excludedDescription : (descriptions?.[choice] ?? DESCRIPTIONS[choice])
          }
          onSelect={() => onChange(choice)}
        />
      ))}
    </MenuButton>
  );
}
