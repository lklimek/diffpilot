import { Fragment, useEffect, useId, useRef, type ReactNode } from "react";
import { buttonClass } from "../../button/button";
import { HelpTip } from "../../settings/HelpTip";
import { BUILT_IN_BRANCH_TEXT, BUILT_IN_CHECK_TEXT, type HelpTopic } from "../../settings/settingsHelp";
import { Tooltip } from "../../tooltip/Tooltip";
import { ERROR, FIELD } from "./look";
import { UnreachableMark, isUnreachable } from "./UnreachableMark";
import { EMPTY_ROW, type FieldErrors, type RepositoryDraft } from "./validation";

const MAX_ROWS = 50;

const COLUMNS: [string, HelpTopic][] = [
  ["Repository", "repo"],
  ["Short name", "alias"],
  ["Watch issues", "watch_issues"],
  ["Ownership policy", "ownership_policy"],
  ["Release branches", "branch_patterns"],
  ["Ignored checks", "ignored_checks"],
];

const BRANCH_HINT =
  "Leave empty for the built-ins. X.Y stands for a version (v1.8-dev matches vX.Y-dev); * matches any text. Your list replaces the built-ins.";

const CHECK_HINT =
  "Ignored checks never make CI failing or running. Separate names with commas; a name matches the whole check name in any case, * matches any text. Leave empty for the built-ins; your list replaces them.";

interface RepositoryTableProps {
  rows: RepositoryDraft[];
  installations?: { name: string; live: boolean | null }[];
  installationStatusKnown?: boolean;
  expanded?: string | null;
  details?: ReactNode;
  onExpand?: (name: string | null) => void;
  onChange: (rows: RepositoryDraft[]) => void;
  /** Errors of the form, keyed `repositories.<index>.<field>` (plan §F.6 G3–G7). */
  errors: FieldErrors;
  /** Repositories the GitHub token could not see at the latest refresh. */
  unreachable: string[];
}

/** One row per repository with its short name, issue watch, ownership policy, release branches and ignored checks. */
export function RepositoryTable({
  rows,
  onChange,
  errors,
  unreachable,
  installations = [],
  installationStatusKnown,
  expanded,
  details,
  onExpand,
}: RepositoryTableProps) {
  const id = useId();
  const opened = useRef<HTMLButtonElement>(null);
  const nextDraftId = useRef(0);
  const installationByName = new Map(installations.map((entry) => [entry.name.toLowerCase(), entry]));
  const configured = new Set(rows.map((row) => row.repo.trim().toLowerCase()));
  const entries: { row: RepositoryDraft; index: number }[] = [
    ...rows.map((row, index) => ({ row, index })),
    ...installations
      .filter(({ name }) => !configured.has(name.toLowerCase()))
      .map(({ name }) => ({
        row: {
          ...EMPTY_ROW,
          id: `installed:${name.toLowerCase()}`,
          origin: { kind: "installed" as const, name },
          repo: name,
        },
        index: -1,
      })),
  ];
  const expandedKnown =
    !!expanded && entries.some(({ row }) => row.repo.trim().toLowerCase() === expanded.toLowerCase());
  useEffect(() => {
    if (expandedKnown) opened.current?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [expanded, expandedKnown]);
  const full = rows.length >= MAX_ROWS;
  return (
    <div className="grid content-start gap-2">
      {expanded && !expandedKnown && (
        <p role="status">Repository {expanded} not found in this installation.</p>
      )}
      {errors.repositories && <p className={ERROR}>{errors.repositories}</p>}
      {entries.length === 0 ? (
        <p className="text-xs text-muted">
          No repositories yet. Pull requests and issues of yours are still found by search.
        </p>
      ) : (
        <div
          role="region"
          aria-label="Scrollable repository table"
          tabIndex={0}
          className="relative min-w-0 overflow-x-auto"
        >
          {/* `relative`: the cells' screen-reader text (absolutely placed) scrolls with the table, not the page. */}
          <table className="w-full border-collapse text-sm">
            <caption className="sr-only">Repositories</caption>
            <thead>
              <tr>
                {COLUMNS.map(([name, topic], index) => (
                  <Fragment key={name}>
                    <th
                      key={name}
                      scope="col"
                      className="border-b border-border px-2 py-1.5 text-left text-[11px] tracking-[.4px] whitespace-nowrap text-muted uppercase"
                    >
                      <span className="inline-flex items-center gap-1.5">
                        {name}
                        <HelpTip topic={topic} />
                      </span>
                    </th>
                    {index === 0 && (
                      <th scope="col" className="border-b border-border px-2 text-left">
                        Live
                      </th>
                    )}
                  </Fragment>
                ))}
                <th scope="col" className="border-b border-border px-2 text-left">
                  Analysis rules
                </th>
                <th scope="col" className="border-b border-border">
                  <span className="sr-only">Remove</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {entries.map(({ row, index }, i) => {
                const name = row.repo.trim() || `row ${i + 1}`;
                const installation = installationByName.get(name.toLowerCase());
                const readOnlyName =
                  row.origin !== null &&
                  (row.origin.kind === "installed" || installationByName.has(row.origin.name.toLowerCase()));
                const duplicate = entries
                  .slice(0, i)
                  .some(({ row: earlier }) => earlier.repo.trim().toLowerCase() === name.toLowerCase());
                const live = installation ? installation.live : installationStatusKnown ? false : null;
                const openedRow = !duplicate && expanded?.toLowerCase() === name.toLowerCase();
                const detailsId = `${id}-${i}-details`;
                const set = (change: Partial<RepositoryDraft>) => {
                  if (index < 0 && full) return;
                  const next = { ...row, ...change };
                  onChange(
                    index < 0 ? [...rows, next] : rows.map((existing, j) => (j === index ? next : existing)),
                  );
                };
                const bare = row.repo.split("/")[1]?.trim();
                const error = (field: string) => errors[`repositories.${index}.${field}`];
                const described = (field: string) => (error(field) ? `${id}-${i}-${field}` : undefined);
                const policyHint = `${id}-${i}-policy-hint`;
                const policyDescribed = [policyHint, described("policy")].filter(Boolean).join(" ");
                const message = (field: string) =>
                  error(field) && (
                    <p id={`${id}-${i}-${field}`} className={ERROR}>
                      {error(field)}
                    </p>
                  );
                return (
                  <Fragment key={row.id}>
                    <tr className="align-top">
                      <td className="border-b border-border px-2 py-1.5">
                        <div className="grid gap-1">
                          {readOnlyName ? (
                            <span className="font-mono">{name}</span>
                          ) : (
                            <input
                              aria-label={`Repository ${i + 1}`}
                              value={row.repo}
                              placeholder="owner/name"
                              autoComplete="off"
                              aria-invalid={error("repo") ? true : undefined}
                              aria-describedby={described("repo")}
                              className={`${FIELD} w-48 font-mono`}
                              onChange={(e) => set({ repo: e.target.value })}
                            />
                          )}
                          {isUnreachable(unreachable, row.repo.trim()) && <UnreachableMark name={name} />}
                          {message("repo")}
                        </div>
                      </td>
                      <td className="border-b border-border px-2 py-1.5 text-xs">
                        {live === true
                          ? "✓ Live"
                          : live === false
                            ? "App not installed"
                            : "Installation not checked"}
                      </td>
                      <td className="border-b border-border px-2 py-1.5">
                        <input
                          disabled={index < 0 && full}
                          aria-label={`Short name of ${name}`}
                          value={row.alias}
                          placeholder={bare || "repository name"}
                          autoComplete="off"
                          aria-invalid={error("alias") ? true : undefined}
                          aria-describedby={described("alias")}
                          className={`${FIELD} w-28 font-mono`}
                          onChange={(e) => set({ alias: e.target.value.toUpperCase() })}
                        />
                        {message("alias")}
                      </td>
                      <td className="border-b border-border px-2 py-1.5 text-center">
                        <input
                          type="checkbox"
                          disabled={index < 0 && full}
                          aria-label={`Watch issues of ${name}`}
                          checked={row.watch_issues}
                          className="mt-1 size-6"
                          onChange={(e) => set({ watch_issues: e.target.checked })}
                        />
                      </td>
                      <td className="border-b border-border px-2 py-1.5">
                        <div className="grid gap-1">
                          <input
                            disabled={index < 0 && full}
                            aria-label={`Policy repository of ${name}`}
                            value={row.policy_repo}
                            placeholder="owner/name"
                            autoComplete="off"
                            aria-invalid={error("policy") ? true : undefined}
                            aria-describedby={policyDescribed}
                            className={`${FIELD} w-44 font-mono`}
                            onChange={(e) => set({ policy_repo: e.target.value })}
                          />
                          <input
                            disabled={index < 0 && full}
                            aria-label={`Policy path of ${name}`}
                            value={row.policy_path}
                            placeholder="policies/<repo>.json"
                            autoComplete="off"
                            aria-invalid={error("policy") ? true : undefined}
                            aria-describedby={policyDescribed}
                            className={`${FIELD} w-44 font-mono`}
                            onChange={(e) => set({ policy_path: e.target.value })}
                          />
                          <p id={policyHint} className="m-0 text-[11px] text-muted">
                            Optional — leave empty if there is no policy file
                          </p>
                          {message("policy")}
                        </div>
                      </td>
                      <td className="border-b border-border px-2 py-1.5">
                        <input
                          disabled={index < 0 && full}
                          aria-label={`Release branches of ${name}`}
                          value={row.branch_patterns}
                          placeholder={`empty = ${BUILT_IN_BRANCH_TEXT}`}
                          autoComplete="off"
                          aria-invalid={error("branch_patterns") ? true : undefined}
                          aria-describedby={[`${id}-branch-hint`, described("branch_patterns")]
                            .filter(Boolean)
                            .join(" ")}
                          className={`${FIELD} w-full min-w-56 font-mono`}
                          onChange={(e) => set({ branch_patterns: e.target.value })}
                        />
                        {row.branch_patterns.trim() === "" && (
                          <p className="m-0 mt-1 font-mono text-[11px] text-muted">
                            Built-ins: {BUILT_IN_BRANCH_TEXT}
                          </p>
                        )}
                        {message("branch_patterns")}
                      </td>
                      <td className="border-b border-border px-2 py-1.5">
                        <input
                          disabled={index < 0 && full}
                          aria-label={`Ignored checks of ${name}`}
                          value={row.ignored_checks}
                          placeholder={BUILT_IN_CHECK_TEXT}
                          autoComplete="off"
                          aria-invalid={error("ignored_checks") ? true : undefined}
                          aria-describedby={[`${id}-check-hint`, described("ignored_checks")]
                            .filter(Boolean)
                            .join(" ")}
                          className={`${FIELD} w-full min-w-56 font-mono`}
                          onChange={(e) => set({ ignored_checks: e.target.value })}
                        />
                        {row.ignored_checks.trim() === "" && (
                          <p className="m-0 mt-1 font-mono text-[11px] text-muted">
                            Built-ins: {BUILT_IN_CHECK_TEXT}
                          </p>
                        )}
                        {message("ignored_checks")}
                      </td>
                      <td className="border-b border-border px-2 py-1.5">
                        {row.repo.includes("/") && onExpand && (
                          <Tooltip
                            tip={
                              openedRow
                                ? "Hides the read-only analysis rules."
                                : "Shows the read-only analysis rules for this repository."
                            }
                          >
                            <button
                              ref={openedRow ? opened : undefined}
                              type="button"
                              aria-label={`${openedRow ? "Hide" : "Show"} analysis rules for ${name}`}
                              aria-expanded={openedRow}
                              aria-controls={openedRow ? detailsId : undefined}
                              className={buttonClass("plain", true)}
                              onClick={() => onExpand(openedRow ? null : name)}
                            >
                              {openedRow ? "Hide" : "Show"}
                            </button>
                          </Tooltip>
                        )}
                      </td>
                      <td className="border-b border-border px-2 py-1.5">
                        {index >= 0 && (
                          <Tooltip tip={`Removes the editable rules for ${name}. Save to apply.`}>
                            <button
                              type="button"
                              aria-label={`Remove ${name}`}
                              className={buttonClass("plain", true)}
                              onClick={() => onChange(rows.filter((_, j) => j !== index))}
                            >
                              Remove
                            </button>
                          </Tooltip>
                        )}
                      </td>
                    </tr>
                    {openedRow && (
                      <tr id={detailsId}>
                        <td colSpan={9} className="border-b border-border px-2 py-2">
                          <div className="sticky left-0 w-[min(100%,calc(100vw-96px))] max-w-[900px]">
                            {details}
                          </div>
                        </td>
                      </tr>
                    )}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
          <p id={`${id}-branch-hint`} className="m-0 mt-1.5 text-xs text-muted">
            {BRANCH_HINT}
          </p>
          <p id={`${id}-check-hint`} className="m-0 mt-1 text-xs text-muted">
            {CHECK_HINT}
          </p>
        </div>
      )}
      <Tooltip
        tip={
          full
            ? `At most ${MAX_ROWS} repositories. Remove one to add another.`
            : "Adds an empty row for a repository. Save to apply."
        }
      >
        <button
          type="button"
          aria-disabled={full || undefined}
          className={`${buttonClass("plain", true)} w-fit`}
          onClick={() => {
            if (!full)
              onChange([...rows, { ...EMPTY_ROW, id: `new:${nextDraftId.current++}`, origin: null }]);
          }}
        >
          Add repository
        </button>
      </Tooltip>
    </div>
  );
}
