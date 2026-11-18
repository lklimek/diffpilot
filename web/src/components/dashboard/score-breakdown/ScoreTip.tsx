import type { WorkItem } from "../../../api/generated/types";
import { dayText, pointsText } from "../common/format";
import { partLabel } from "./parts";

const TOP = 5;

/** Headline of a score: what the number is and whether it is final. */
export function scoreHeadline(item: WorkItem): string {
  const points = Math.round(item.score.total_tenths / 10);
  if (item.score.state === "unjudged") {
    return `Provisional ~${points}: computed parts and fixed estimates; the LLM has not judged this item yet`;
  }
  if (item.score.state === "provisional") {
    const since = item.llm ? ` since the ${dayText(item.llm.judged_at)} judgement` : "";
    return `Provisional ~${points}: changed${since}; LLM parts are carried over until re-judged`;
  }
  return `Score ${points} = sum of ${item.score.parts.length} parts`;
}

/** Tooltip of a score: its headline and its largest parts with their source (`SR_TIPS.score` of the mockup). */
export function ScoreTip({ item }: { item: WorkItem }) {
  const top = item.score.parts
    .filter((part) => part.points_tenths)
    .sort((a, b) => Math.abs(b.points_tenths ?? 0) - Math.abs(a.points_tenths ?? 0))
    .slice(0, TOP);
  return (
    <span className="grid gap-1">
      <b>{scoreHeadline(item)}</b>
      <table className="border-collapse text-[11.5px]">
        <tbody>
          {top.map((part) => (
            <tr key={part.component}>
              <td className="pr-2.5">
                {partLabel(part.component)}
                {part.status === "estimated" ? (
                  <span className="opacity-75"> (estimate)</span>
                ) : (
                  part.source === "llm" && <span className="opacity-75"> (LLM)</span>
                )}
              </td>
              <td className="text-right font-mono">{pointsText(part.points_tenths ?? 0)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <span className="opacity-75">Click for all parts and their basis.</span>
    </span>
  );
}
