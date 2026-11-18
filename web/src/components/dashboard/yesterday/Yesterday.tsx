import { signed } from "../common/format";
import { AsideSection } from "../side-panel/AsideSection";

interface YesterdayProps {
  /** The previous working day (`Tue 17 Nov`). */
  label: string;
  /** The user's PRs closed (merged or not), of which merged, and PRs opened. */
  prs: { closed: number; merged: number; opened: number };
  /** Reviews the scanned logins gave. */
  reviews: number;
}

/** The previous working day: PRs closed / opened and reviews given (OD16: pull requests and reviews only). */
export function Yesterday({ label, prs, reviews }: YesterdayProps) {
  const net = prs.closed - prs.opened;
  return (
    <AsideSection
      title="Yesterday"
      summary={
        <>
          PRs net <b>{signed(net)}</b> · {reviews} reviews
        </>
      }
    >
      <table className="w-full border-collapse overflow-hidden rounded-md border border-border text-[12.5px] tabular-nums [&_td]:border-b [&_td]:border-border [&_td]:px-2 [&_td]:py-1 [&_td]:text-right [&_th]:border-b [&_th]:border-border [&_th]:px-2 [&_th]:py-1 [&_th]:text-right">
        <caption className="sr-only">Previous working day, {label}</caption>
        <thead className="bg-surface-2 text-[10.5px] tracking-wide text-muted uppercase">
          <tr>
            <th scope="col" className="!text-left">
              {label}
            </th>
            <th scope="col">Closed</th>
            <th scope="col">Opened</th>
            <th scope="col">Net</th>
            <th scope="col">Note</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <th scope="row" className="!text-left font-semibold">
              PRs
            </th>
            <td>{prs.closed}</td>
            <td>{prs.opened}</td>
            <td>
              <b>{signed(net)}</b>
            </td>
            <td className="text-xs text-muted">{prs.merged} merged</td>
          </tr>
          <tr>
            <th scope="row" className="!text-left font-semibold">
              Reviews
            </th>
            <td>{reviews}</td>
            <td>—</td>
            <td>—</td>
            <td className="text-xs text-muted">given</td>
          </tr>
        </tbody>
      </table>
    </AsideSection>
  );
}
