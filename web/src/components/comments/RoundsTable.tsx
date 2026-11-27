import type { DecisionKind, Round } from "../../api/generated/types";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { dateTimeText } from "../format/time";
import { ProvenanceTag } from "../section/ProvenanceTag";

const TH = "border-b border-border px-2 py-1.5 text-left text-[11px] font-semibold text-muted uppercase";
const TD = "border-b border-border px-2 py-1.5";
const N = `${TD} text-right tabular-nums`;

const count = (round: Round, kinds: DecisionKind[]) =>
  round.decisions.filter((d) => kinds.includes(d.kind)).reduce((n, d) => n + d.count, 0);

/** V4: per bot round its head, end, verdict and counts (plan §3.5); counts only. */
export function RoundsTable({ rounds, prNumber }: { rounds: Round[]; prNumber: number }) {
  if (rounds.length === 0)
    return <p className="text-muted">No bot round finished on this pull request yet.</p>;
  return (
    <>
      <table className="w-full border-collapse text-[13px]">
        <caption className="sr-only">Bot rounds of #{prNumber}</caption>
        <thead>
          <tr>
            <th scope="col" className={TH}>
              Round
            </th>
            <th scope="col" className={TH}>
              Head
            </th>
            <th scope="col" className={TH}>
              Finished
            </th>
            <th scope="col" className={TH}>
              Bot verdict <ProvenanceTag source="github" />
            </th>
            {[
              "New",
              "Still open",
              "Carried",
              "Asked again",
              "Fix",
              "Reject",
              "Moved / follow-up",
              "Undecided",
            ].map((h) => (
              <th key={h} scope="col" className={`${TH} text-right`}>
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rounds.map((r) => (
            <tr key={r.number}>
              <td className={TD}>
                {r.number} <OpenExternal href={r.url} label={`Open round ${r.number} on GitHub`} />
              </td>
              <td className={TD}>
                <code>{r.head.slice(0, 7)}</code>
              </td>
              <td className={TD}>{dateTimeText(r.finished_at)}</td>
              <td className={TD}>{r.verdict.toLowerCase().replaceAll("_", " ")}</td>
              <td className={N}>{r.new}</td>
              <td className={N}>{r.still_open}</td>
              <td className={N}>{r.carried}</td>
              <td className={N}>{r.asked_again}</td>
              <td className={N}>{count(r, ["fix_here"])}</td>
              <td className={N}>{count(r, ["reject"])}</td>
              <td className={N}>
                {count(r, ["handled_in_pr", "follow_up_pr", "follow_up_issue", "upstream_issue", "todo"])}
              </td>
              <td className={N}>{r.undecided}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="mt-2 text-xs text-muted">
        A round is a thepastaclaw review whose body carries its final-phase marker for that head.
      </p>
    </>
  );
}
