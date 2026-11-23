import type { CarryEvent, CarryOverPage, PrRef } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { clockText, dateTimeText } from "../format/time";
import { prGithubUrl } from "../github/links";
import { Tooltip } from "../tooltip/Tooltip";
import { CHOICE_LABEL } from "./decisions";

const TH = "border-b border-border px-2 py-1.5 text-left text-[11px] font-semibold text-muted uppercase";
const TD = "border-b border-border px-2 py-1.5 align-top";
const prName = (pr: PrRef) => `${pr.repo.owner}/${pr.repo.name} #${pr.number}`;

function why(event: CarryEvent): string {
  if (event.kind === "carried") return "same marker; comment not edited; code unchanged";
  if (event.reason === "bot_edited") return "the bot edited its comment (text changed)";
  const lines = event.evidence.lines;
  const commit = event.evidence.commit?.slice(0, 7);
  return `code${lines ? ` at lines ${lines.start_line}-${lines.end_line}` : ""} changed${commit ? ` in ${commit}` : ""}`;
}

interface CarryOversProps {
  page: CarryOverPage;
  /** One PR ("This pull request") or all. */
  scope: "pr" | "all";
  /** The scope switch's targets (in-app). */
  prHref: string | null;
  allHref: string;
  onNavigate: Navigate;
  /** Questions with the item open (this PR's scope only). */
  itemHref?: (no: number) => string;
  /** Asks a carried item again (this PR's scope only); absent: no Reopen. */
  onReopen?: (event: CarryEvent) => void;
  /** Loads the next older events (`page.next_cursor` is set); `loadingOlder` while it runs. */
  onShowOlder?: () => void;
  loadingOlder?: boolean;
  now: Date;
}

/**
 * V5: tiles of counts and the dated list (one PR, newest first, older events on request), or one row per PR (all);
 * counts only, no percentages.
 */
export function CarryOvers({
  page,
  scope,
  prHref,
  allHref,
  onNavigate,
  itemHref,
  onReopen,
  onShowOlder,
  loadingOlder = false,
  now,
}: CarryOversProps) {
  const t = page.totals;
  const tiles: [number, string][] = [
    [t.carried, "carried without asking"],
    [t.asked_again_bot_edited + t.asked_again_code_changed, "asked again"],
    [t.asked_again_bot_edited, "… the bot edited its comment"],
    [t.asked_again_code_changed, "… the code at that place changed"],
    [t.effects_pending, "carried effects open"],
  ];
  const seg =
    "min-h-6 cursor-pointer border-0 px-3 py-1 no-underline aria-[current=page]:bg-accent aria-[current=page]:font-semibold aria-[current=page]:text-on-accent";
  return (
    <div>
      <nav
        aria-label="Scope"
        className="mb-2.5 inline-flex overflow-hidden rounded-md border border-border bg-surface"
      >
        {prHref && (
          <a
            href={prHref}
            aria-current={scope === "pr" ? "page" : undefined}
            onClick={navigateOnClick(prHref, onNavigate)}
            className={`${seg} text-text`}
          >
            This pull request
          </a>
        )}
        <a
          href={allHref}
          aria-current={scope === "all" ? "page" : undefined}
          onClick={navigateOnClick(allHref, onNavigate)}
          className={`${seg} border-l border-border text-text`}
        >
          All pull requests
        </a>
      </nav>
      <p className="text-xs text-muted">
        A repeat of a decided finding (same <code>dedupe=</code> marker) takes your earlier decision without a
        question. It is asked again only when the bot edited its comment text or the code at that place
        changed after your decision. Counts only.
      </p>
      <ul
        aria-label="Carry-over counts"
        className="m-0 mb-3.5 grid list-none grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-2.5 p-0"
      >
        {tiles.map(([v, l]) => (
          <li key={l} className="rounded-lg border border-border bg-surface px-3 py-2">
            <div className="text-2xl font-bold tabular-nums">{v}</div>
            <div className="text-xs text-muted">{l}</div>
          </li>
        ))}
      </ul>
      {scope === "all" ? (
        <table className="w-full border-collapse text-[13px]">
          <caption className="sr-only">Carry-overs per pull request</caption>
          <thead>
            <tr>
              {[
                "Pull request",
                "Carried",
                "Asked again: bot edit",
                "Asked again: code changed",
                "Effects open",
              ].map((h) => (
                <th key={h} scope="col" className={TH}>
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {page.per_pr.map((row) => (
              <tr key={prName(row.pr)}>
                <td className={TD}>
                  {prName(row.pr)}{" "}
                  <OpenExternal href={prGithubUrl(row.pr)} label={`Open ${prName(row.pr)} on GitHub`} />
                </td>
                <td className={TD}>{row.counts.carried}</td>
                <td className={TD}>{row.counts.asked_again_bot_edited}</td>
                <td className={TD}>{row.counts.asked_again_code_changed}</td>
                <td className={TD}>{row.counts.effects_pending}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : page.events.length === 0 ? (
        <p className="text-muted">No repeat was carried or asked again on this pull request.</p>
      ) : (
        <table className="w-full border-collapse text-[13px]">
          <caption className="sr-only">Carry-over events</caption>
          <thead>
            <tr>
              {["When", "Item", "Repeat of", "Event", "Why", "Effects", ...(onReopen ? ["Action"] : [])].map(
                (h) => (
                  <th key={h} scope="col" className={TH}>
                    {h}
                  </th>
                ),
              )}
            </tr>
          </thead>
          <tbody>
            {page.events.map((e) => (
              <tr key={e.id}>
                <td className={TD} title={dateTimeText(e.at)}>
                  {clockText(e.at, now)}
                </td>
                <td className={TD}>
                  {itemHref && e.kind === "asked_again" ? (
                    <a href={itemHref(e.item_no)} onClick={navigateOnClick(itemHref(e.item_no), onNavigate)}>
                      C{e.item_no}
                    </a>
                  ) : (
                    `C${e.item_no}`
                  )}{" "}
                  <OpenExternal href={e.evidence.url} label={`Open C${e.item_no} on GitHub`} />
                </td>
                <td className={TD}>
                  C{e.source_item_no} · {CHOICE_LABEL[e.source_kind] ?? e.source_kind}
                </td>
                <td className={TD}>{e.kind === "carried" ? "carried" : "asked again"}</td>
                <td className={TD}>{why(e)}</td>
                <td className={TD}>{e.effects_pending ? `${e.effects_pending} open` : "—"}</td>
                {onReopen && (
                  <td className={TD}>
                    {e.kind === "carried" && (
                      <Tooltip
                        tip={`Asks C${e.item_no} again: effects of the carried decision not yet claimed are cancelled.`}
                      >
                        <button
                          type="button"
                          onClick={() => onReopen(e)}
                          className={buttonClass("plain", true)}
                        >
                          Reopen
                        </button>
                      </Tooltip>
                    )}
                  </td>
                )}
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {scope === "pr" && page.next_cursor && (
        <p className="mt-2 flex flex-wrap items-center gap-2 text-sm">
          <span className="text-muted">The {page.events.length} newest events are shown.</span>
          {onShowOlder && (
            <Tooltip tip="Loads the next older carry-over events of this pull request.">
              <button
                type="button"
                aria-disabled={loadingOlder || undefined}
                onClick={() => !loadingOlder && onShowOlder()}
                className={buttonClass("plain", true)}
              >
                {loadingOlder ? "Loading older events…" : "Show older events"}
              </button>
            </Tooltip>
          )}
        </p>
      )}
    </div>
  );
}
