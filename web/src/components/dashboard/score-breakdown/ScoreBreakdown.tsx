import type { WorkItem } from "../../../api/generated/types";
import { ProvenanceTag } from "../../section/ProvenanceTag";
import { Tooltip } from "../../tooltip/Tooltip";
import { dayText, daysSince, pointsText, tierName } from "../common/format";
import { partLabel, sourceView } from "./parts";

// 30 points (the largest part) draw a 60 px bar.
const BAR_PX_PER_TENTH = 60 / 300;
const H3 = "mt-2.5 mb-1.5 text-[11px] font-bold tracking-wide text-muted uppercase";

function tierText(item: WorkItem): string {
  const points = Math.round(item.score.total_tenths / 10);
  if (item.score.state === "unjudged") return "not judged yet";
  const tier =
    item.score.tier === "p0" ? `P0 alarm (score ${points} kept for reference)` : tierName(item.score.tier);
  return item.score.state === "provisional" ? `${tier}, provisional` : tier;
}

/**
 * Detail of a ranked item: every score part with its points, basis and source, the tier, and the scan facts
 * (mockup `detailHTML`).
 */
export function ScoreBreakdown({ item, now }: { item: WorkItem; now: Date }) {
  const judgedOn = item.llm ? dayText(item.llm.judged_at) : null;
  const final = item.score.state === "final";
  const updated = daysSince(item.updated_at, now);
  return (
    <div className="grid grid-cols-1 gap-x-7 gap-y-3.5 @min-[1100px]:grid-cols-2">
      <div className="min-w-0">
        <h3 className={H3}>Score breakdown</h3>
        <table className="w-full border-collapse rounded-md border border-border bg-surface text-[12.5px]">
          <thead>
            <tr className="text-[10.5px] tracking-wide text-muted uppercase [&>th]:border-b [&>th]:border-border [&>th]:px-2 [&>th]:py-1.5 [&>th]:text-left">
              <th scope="col">Part</th>
              <th scope="col" className="!text-right">
                Points
              </th>
              <th scope="col">
                <span className="sr-only">Bar</span>
              </th>
              <th scope="col">Basis</th>
              <th scope="col">Source</th>
            </tr>
          </thead>
          <tbody className="[&_td]:border-b [&_td]:border-border [&_td]:px-2 [&_td]:py-1 [&_td]:align-top">
            {item.score.parts.map((part) => {
              const source = sourceView(part, judgedOn);
              const tenths = part.points_tenths;
              return (
                <tr key={part.component} data-part={part.component}>
                  <td>{partLabel(part.component)}</td>
                  <td className="text-right font-mono whitespace-nowrap tabular-nums">
                    {tenths === null ? "—" : pointsText(tenths)}
                  </td>
                  <td className="w-[70px]">
                    {tenths ? (
                      <span
                        aria-hidden
                        className={`inline-block h-1.5 rounded-sm ${tenths < 0 ? "bg-c-neg" : "bg-c-pos"}`}
                        style={{ width: Math.round(Math.abs(tenths) * BAR_PX_PER_TENTH) }}
                      />
                    ) : null}
                  </td>
                  <td>{part.basis}</td>
                  <td className="text-[11px] whitespace-nowrap text-muted" data-source>
                    {source.llm && <ProvenanceTag source="llm" focusable={false} />}{" "}
                    {source.note && (
                      <Tooltip tip={source.tip}>
                        <span tabIndex={0} className="cursor-help">
                          {source.note}
                        </span>
                      </Tooltip>
                    )}
                  </td>
                </tr>
              );
            })}
            <tr className="font-bold [&>td]:border-b-0">
              <td>Score</td>
              <td className="text-right font-mono tabular-nums">
                {final ? "" : "~"}
                {pointsText(item.score.total_tenths).replace(/^\+/, "")}
              </td>
              <td />
              <td colSpan={2}>{tierText(item)}</td>
            </tr>
          </tbody>
        </table>
        <p className="mt-1.5 text-xs text-muted">
          Tiers come only from the sum: P1 ≥ 65, P2 35–&lt;65, P3 &lt; 35. A P0 alarm is a separate LLM
          judgement, never a score.
        </p>
      </div>
      <div className="min-w-0">
        <h3 className={H3}>Facts from {item.kind === "todo" ? "MemCan" : "GitHub"}</h3>
        <dl className="m-0 grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-[12.5px] [&_dd]:m-0 [&_dt]:pt-0.5 [&_dt]:text-[11px] [&_dt]:font-bold [&_dt]:tracking-wide [&_dt]:text-muted [&_dt]:uppercase">
          {item.facts.map((fact, i) => (
            <div key={`${fact.label}-${i}`} className="contents">
              <dt>{fact.label}</dt>
              <dd>{fact.value}</dd>
            </div>
          ))}
          <dt>{item.kind === "todo" ? "Changed" : "Updated"}</dt>
          <dd>{updated === 0 ? "today" : `${updated} d ago`}</dd>
        </dl>
        <h3 className={H3}>Read-only</h3>
        <p className="text-xs text-muted">
          No labels, comments or assignments are written to GitHub, and no TODO is changed in MemCan, from
          this page.
        </p>
      </div>
    </div>
  );
}
