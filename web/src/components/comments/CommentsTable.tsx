import { Tooltip } from "../tooltip/Tooltip";
import { useId, useState } from "react";
import type { CommentItem, DecisionBatchRow, DecisionVia } from "../../api/generated/types";
import type { ReviewComment } from "../../api/provisional/comments";
import { GithubDecision } from "./GithubDecision";
import { commentStatus } from "./queueStatus";
import { buttonClass } from "../button/button";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { clockText } from "../format/time";
import { ProvenanceTag } from "../section/ProvenanceTag";
import {
  CHOICE_LABEL,
  choicesFor,
  decisionLabel,
  gateOf,
  missingInput,
  needOf,
  recommendedValue,
  requestOf,
  resolveLocked,
  type Choice,
} from "./decisions";
import { CodeText } from "../section/CodeText";
import { GateChip, itemId, validityAt } from "./parts";

const VIA: Record<DecisionVia, string> = {
  question: "in Questions",
  group: "in a group",
  bulk: "via decide the rest",
  table: "in the table",
  carried: "carried",
};

interface Row {
  choice: Choice | "";
  value: string;
  resolve: boolean;
  comment: string;
  /** The item's revision the row was filled in against. */
  revision: number;
}

const initial = (item: CommentItem): Row => {
  const rec = item.assessment?.recommendation.kind;
  return {
    choice: gateOf(item) || !rec ? "" : rec,
    value: gateOf(item) ? "" : recommendedValue(item),
    resolve: true,
    comment: "",
    revision: item.revision,
  };
};

interface CommentsTableProps {
  /** PR number and purpose for the line above the table. */
  prNumber: number;
  purpose: string | null;
  items: ReviewComment[];
  /** The pull request's head now: an assessment of another commit names that commit. */
  prHead: string;
  /** Ask answers per item (live). */
  askAnswers: (item: CommentItem) => number;
  /** Questions with the item open (`?item=C14`). */
  itemHref: (item: CommentItem) => string;
  onNavigate: Navigate;
  /** Opens the Ask chat at the item's threads; absent without Ask. */
  onOpenAsk?: (item: CommentItem) => void;
  onSubmit: (rows: DecisionBatchRow[]) => void;
  busy: boolean;
  error: string | null;
  now: Date;
}

const TH =
  "border-b border-border bg-surface px-2 py-1.5 text-left text-[11px] font-semibold tracking-wide text-muted uppercase";
const TD = "border-b border-border px-2 py-1.5 align-top";

/**
 * The table page (V2, RQ21): `check-pr-comments` §4 columns, a decision select per row (default = recommendation,
 * empty for gate rows), the reason or target, Resolve, the owner's comment and Ask answers; one submit. Rows keep
 * what the reader typed when items are read again; a row whose item changed since is marked and needs a look.
 */
export function CommentsTable(props: CommentsTableProps) {
  const { items, now } = props;
  const missingId = useId();
  const isOpen = (i: ReviewComment) => ["needs_you", "not_assessed"].includes(commentStatus(i));
  const open = items.filter((i) => isOpen(i) && i.kind !== "pr_question");
  const closed = items.filter((i) => !isOpen(i) && i.state !== "assessing" && i.kind !== "pr_question");
  const [rows, setRows] = useState<Record<string, Row>>({});
  const [showAll, setShowAll] = useState(false);
  const rowOf = (item: CommentItem) => rows[item.id] ?? initial(item);
  const set = (item: CommentItem, change: Partial<Row>) =>
    setRows({ ...rows, [item.id]: { ...rowOf(item), ...change } });
  const changed = (item: CommentItem) => rowOf(item).revision !== item.revision;

  const missing: string[] = [];
  const counts = new Map<string, number>();
  for (const item of open) {
    const row = rowOf(item);
    if (changed(item)) missing.push(`${itemId(item)} (changed since you started: check the row)`);
    if (!row.choice) {
      missing.push(
        `${itemId(item)} (${gateOf(item) ? "gate item: choose explicitly" : "choose a decision"})`,
      );
      continue;
    }
    if (missingInput({ choice: row.choice, value: row.value }))
      missing.push(`${itemId(item)} (reason or target)`);
    counts.set(CHOICE_LABEL[row.choice], (counts.get(CHOICE_LABEL[row.choice]) ?? 0) + 1);
  }
  const total = [...counts.values()].reduce((a, b) => a + b, 0);
  const submit = () => {
    if (props.busy || missing.length || total === 0) return;
    props.onSubmit(
      open.map((item) => {
        const row = rowOf(item);
        return {
          item_no: item.no,
          ...requestOf({ ...item, revision: row.revision }, { ...row, choice: row.choice as Choice }, null),
        };
      }),
    );
  };

  const shownClosed = showAll ? closed : closed.slice(0, 2);
  const problem = (item: CommentItem) => (
    <a href={props.itemHref(item)} onClick={navigateOnClick(props.itemHref(item), props.onNavigate)}>
      {item.title}
    </a>
  );
  const ask = (item: CommentItem) => {
    const n = props.askAnswers(item);
    if (!n) return <span className="text-muted">—</span>;
    return props.onOpenAsk ? (
      <Tooltip tip="Opens the Ask chat about this comment.">
        <button type="button" onClick={() => props.onOpenAsk!(item)} className={buttonClass("plain", true)}>
          {n} {n === 1 ? "answer" : "answers"}
        </button>
      </Tooltip>
    ) : (
      `${n} ${n === 1 ? "answer" : "answers"}`
    );
  };
  const common = (item: CommentItem) => {
    const a = item.assessment;
    return (
      <>
        <td className={TD}>
          {itemId(item)} <OpenExternal href={item.url} label={`Open ${itemId(item)} on GitHub`} />{" "}
          {item.gates.map((g) => (
            <GateChip key={g.gate} gate={g.gate} />
          ))}
          {item.state === "reopened" && <span className="text-xs text-med"> asked again</span>}
          {item.gone_at && <span className="text-xs text-muted"> not in the latest read from GitHub</span>}
        </td>
        <td className={TD}>
          {item.kind.replace("_", " ")} · {item.author_kind}
        </td>
        <td className={TD}>{problem(item)}</td>
        <td className={TD}>
          {a ? (
            <>
              {a.necessity.verdict}: <CodeText text={a.necessity.reason} />
            </>
          ) : (
            "—"
          )}
        </td>
        <td className={TD}>{a?.elsewhere.length ? a.elsewhere.map((e) => e.reference).join(", ") : "—"}</td>
        <td className={TD}>
          {a
            ? `${validityAt(a.validity, a.head, props.prHead)}${a.pre_existing ? "; pre-existing" : ""}`
            : "not assessed"}
        </td>
        <td className={TD}>{a ? CHOICE_LABEL[a.recommendation.kind] : "—"}</td>
      </>
    );
  };

  return (
    <div>
      <p>
        <b>PR #{props.prNumber}:</b> {props.purpose ?? "No purpose stated yet."}{" "}
        <ProvenanceTag source="llm" />
      </p>
      <p className="text-xs text-muted">
        Click a problem to open it in Questions. Decisions, comments and Ask answers given there show here.
        Your comment is sent to the executing agent and not posted to GitHub.
      </p>
      <div className="overflow-x-auto">
        <table className="w-full border-collapse text-[13px]">
          <caption className="sr-only">Decision table of #{props.prNumber}</caption>
          <thead>
            <tr>
              <th scope="col" className={TH}>
                ID
              </th>
              <th scope="col" className={TH}>
                Source
              </th>
              <th scope="col" className={TH}>
                Problem
              </th>
              <th scope="col" className={TH}>
                Scope <ProvenanceTag source="llm" />
              </th>
              <th scope="col" className={TH}>
                Elsewhere
              </th>
              <th scope="col" className={TH}>
                Opinion <ProvenanceTag source="llm" />
              </th>
              <th scope="col" className={TH}>
                Recommendation <ProvenanceTag source="llm" />
              </th>
              <th scope="col" className={TH}>
                Decision
              </th>
              <th scope="col" className={TH}>
                Reason / target
              </th>
              <th scope="col" className={TH}>
                Resolve
              </th>
              <th scope="col" className={TH}>
                Your comment
              </th>
              <th scope="col" className={TH}>
                Ask
              </th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <th scope="rowgroup" colSpan={12} className={`${TD} bg-surface-2 text-left`}>
                Undecided ({open.length})
              </th>
            </tr>
            {open.map((item) => {
              const row = rowOf(item);
              const need = row.choice ? needOf(row.choice) : null;
              const name = `${itemId(item)} (${item.title})`;
              return (
                <tr key={item.id} data-row={itemId(item)}>
                  {common(item)}
                  <td className={TD}>
                    <select
                      aria-label={`Decision for ${name}${gateOf(item) ? ", gate item: choose explicitly" : ""}`}
                      value={row.choice}
                      onChange={(e) => {
                        const choice = e.target.value as Choice | "";
                        set(item, {
                          choice,
                          value:
                            choice && choice === item.assessment?.recommendation.kind
                              ? recommendedValue(item)
                              : "",
                        });
                      }}
                      className="rounded-md border border-muted bg-surface px-1.5 py-1"
                    >
                      {!row.choice || gateOf(item) ? (
                        <option value="">{gateOf(item) ? "Choose… (gate item)" : "Choose…"}</option>
                      ) : null}
                      {choicesFor(item).map((c) => (
                        <option key={c} value={c}>
                          {CHOICE_LABEL[c]}
                        </option>
                      ))}
                    </select>
                    {changed(item) && (
                      <div role="status" className="mt-1 text-xs text-med">
                        Changed since you started; check this row.{" "}
                        <Tooltip tip="Keeps your input for the changed comment.">
                          <button
                            type="button"
                            onClick={() => set(item, { revision: item.revision })}
                            className={buttonClass("plain", true)}
                          >
                            Keep my input for {itemId(item)}
                          </button>
                        </Tooltip>
                      </div>
                    )}
                  </td>
                  <td className={TD}>
                    <input
                      type="text"
                      aria-label={`Reason or target for ${name}`}
                      value={row.value}
                      disabled={!need}
                      placeholder={need ? "" : "—"}
                      onChange={(e) => set(item, { value: e.target.value })}
                      className="min-w-36 rounded-md border border-muted bg-surface px-1.5 py-1 disabled:opacity-60"
                    />
                  </td>
                  <td className={TD}>
                    {item.kind === "thread" ? (
                      <input
                        type="checkbox"
                        aria-label={`Resolve ${itemId(item)}`}
                        checked={(row.choice !== "" && resolveLocked(row.choice)) || row.resolve}
                        disabled={row.choice !== "" && resolveLocked(row.choice)}
                        onChange={(e) => set(item, { resolve: e.target.checked })}
                      />
                    ) : (
                      <span className="text-muted">no thread</span>
                    )}
                  </td>
                  <td className={TD}>
                    <input
                      type="text"
                      aria-label={`Your comment on ${itemId(item)}`}
                      value={row.comment}
                      onChange={(e) => set(item, { comment: e.target.value })}
                      className="min-w-36 rounded-md border border-muted bg-surface px-1.5 py-1"
                    />
                  </td>
                  <td className={TD}>{ask(item)}</td>
                </tr>
              );
            })}
            <tr>
              <th scope="rowgroup" colSpan={12} className={`${TD} bg-surface-2 text-left`}>
                Decided and other comments ({closed.length})
                {closed.length > 2 && (
                  <Tooltip tip="Shows or hides additional rows.">
                    <button
                      type="button"
                      onClick={() => setShowAll(!showAll)}
                      className={`${buttonClass("plain", true)} ml-2`}
                    >
                      {showAll ? "Show 2" : "Show all"}
                    </button>
                  </Tooltip>
                )}
              </th>
            </tr>
            {shownClosed.map((item) => {
              const d = item.decisions.at(-1);
              return (
                <tr key={item.id} data-row={itemId(item)} className="bg-ok-bg/40">
                  {common(item)}
                  <td className={TD}>
                    {d ? (
                      <>
                        <b>{decisionLabel(d)}</b>
                        <div className="text-xs text-muted">
                          {VIA[d.via] ?? d.via} {clockText(d.decided_at, now)} · {d.decided_by}
                        </div>
                      </>
                    ) : item.imported_decision || item.state === "outside" ? (
                      <GithubDecision item={item} />
                    ) : item.duplicate_of != null ? (
                      <>
                        Same point as C{item.duplicate_of} <ProvenanceTag source="llm" />
                      </>
                    ) : item.status_comment ? (
                      "Status comment"
                    ) : (
                      "—"
                    )}
                  </td>
                  <td className={TD}>
                    {d?.reason ?? (d?.target?.type === "existing_pr" ? `#${d.target.number}` : "—")}
                  </td>
                  <td className={TD}>{d ? (d.resolve_ticked ? "yes" : "no") : "—"}</td>
                  <td className={TD}>{d?.comment ?? <span className="text-muted">—</span>}</td>
                  <td className={TD}>{ask(item)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <p className="mt-2.5 text-sm" aria-live="polite">
        Will record {total} decisions
        {total ? `: ${[...counts].map(([k, n]) => `${k} ${n}`).join(" · ")}` : ""}.
      </p>
      {props.error && (
        <p role="alert" className="text-sm text-high">
          {props.error}
        </p>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <Tooltip tip="Stores one decision per undecided row. Available once every gate row has an explicit choice and every needed reason or target is filled.">
          <button
            type="button"
            aria-disabled={props.busy || missing.length > 0 || total === 0 || undefined}
            aria-describedby={missingId}
            onClick={submit}
            className={buttonClass("primary")}
          >
            Record {total} decisions
          </button>
        </Tooltip>
        <span id={missingId} className="text-xs text-muted">
          {missing.length ? `Still needed: ${missing.join(", ")}` : ""}
        </span>
      </div>
    </div>
  );
}
