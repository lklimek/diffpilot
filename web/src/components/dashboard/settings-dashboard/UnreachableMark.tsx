import { Tooltip } from "../../tooltip/Tooltip";

/** Mark of a repository or owner the GitHub token could not see at the latest refresh (plan §A.1). */
export function UnreachableMark({ name }: { name: string }) {
  return (
    <Tooltip
      tip={`At the latest refresh your GitHub token could not see ${name}. Install the GitHub App there or grant it access, then refresh.`}
    >
      <span
        tabIndex={0}
        data-mark="unreachable"
        className="inline-block cursor-help rounded border border-high-bd bg-high-bg px-1.5 font-sans text-[11px] leading-[18px] font-semibold whitespace-nowrap text-high"
      >
        not visible to your GitHub token
      </span>
    </Tooltip>
  );
}

/** Whether `name` (`owner/repo` or an owner) is in the unreachable list; GitHub names ignore case. */
export const isUnreachable = (unreachable: string[], name: string) =>
  unreachable.some((u) => u.toLowerCase() === name.toLowerCase());
