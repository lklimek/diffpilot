import { useId } from "react";
import type { FileViewedState, PrFile, StepId, Tour, ViewedStatus } from "../../api/generated/types";
import { ProvenanceBadge } from "../badge/Badge";
import { Kbd } from "../help/Kbd";
import { NOT_ANALYSED_LABEL } from "../quiet/labels";
import { Tooltip } from "../tooltip/Tooltip";
import { fileName, fileTicks, treeGroups, unexplainedLines, viewedCount } from "./model";
import { VIEWED_LOOK } from "./ViewedChip";

/** Props of the Files view's file tree. */
export interface FileTreeProps {
  files: readonly PrFile[];
  /** The tour, for the step ticks per file. */
  tour: Tour;
  /** Steps ticked in the Flows view (read-only here). */
  ticked: readonly StepId[];
  /** Path of the open file. */
  current: string | null;
  /** Whether the Viewed marks were read from GitHub. */
  viewedStatus: ViewedStatus;
  filter: string;
  onFilter: (filter: string) => void;
  onSelect: (path: string) => void;
}

const ITEM =
  "block w-full cursor-pointer rounded-md border-l-4 border-transparent px-2 py-1 text-left text-text hover:bg-surface-2 aria-[current]:border-accent aria-[current]:bg-accent-bg aria-[current]:font-semibold";
const SMALL = "block text-[11px] font-normal text-muted";

const STATUS_NOTE: Partial<Record<PrFile["status"], string>> = { added: "new file", deleted: "deleted" };

const dir = (path: string) => (path.includes("/") ? path.slice(0, path.lastIndexOf("/") + 1) : "/");

function Glyph({ state }: { state: FileViewedState | null }) {
  const look = VIEWED_LOOK[state ?? "unread"];
  return (
    <span className="inline-block w-4 shrink-0 text-center">
      <span aria-hidden className={look.className}>
        {look.glyph}
      </span>
      <span className="sr-only">{look.spoken}</span>
    </span>
  );
}

function Row({
  file,
  current,
  tour,
  ticked,
  onSelect,
}: { file: PrFile; current: boolean } & Pick<FileTreeProps, "tour" | "ticked" | "onSelect">) {
  const ticks = fileTicks(tour, file, ticked);
  const unexplained = unexplainedLines(file);
  const notes = [
    file.status === "renamed" && file.old_path ? `renamed from ${file.old_path}` : null,
    STATUS_NOTE[file.status] ?? null,
    file.not_rendered ? "not shown" : null,
    file.viewed === "dismissed" ? "changed since viewed" : null,
  ].filter(Boolean);
  const allTicked = ticks.total > 0 && ticks.ticked === ticks.total;
  return (
    <li>
      <button
        type="button"
        data-file={file.path}
        aria-current={current ? "page" : undefined}
        onClick={() => onSelect(file.path)}
        className={ITEM}
      >
        <span className="flex items-baseline gap-1.5">
          <Glyph state={file.viewed} />
          <span className="min-w-0 flex-1 truncate font-mono text-[12.5px]">
            <span className="sr-only">{dir(file.path)}</span>
            {fileName(file.path)}
          </span>
          <span className="shrink-0 font-mono text-[11px] font-normal">
            {file.added > 0 && <span className="text-add">+{file.added}</span>}
            {file.added > 0 && file.removed > 0 && " "}
            {file.removed > 0 && <span className="text-high">−{file.removed}</span>}
          </span>
        </span>
        <span className={`${SMALL} pl-5.5`}>
          {ticks.total > 0 && (
            <>
              {ticks.flows.length > 0 &&
                !allTicked &&
                `flow${ticks.flows.length === 1 ? "" : "s"} ${ticks.flows.join(", ")} · `}
              {allTicked ? (
                <span className="font-semibold text-ok">
                  <span aria-hidden>☑ </span>
                  {ticks.ticked}/{ticks.total} steps ticked
                </span>
              ) : (
                `${ticks.ticked}/${ticks.total} steps ticked`
              )}
            </>
          )}
          {unexplained > 0 && (
            <>
              {ticks.total > 0 && " · "}
              <span className="font-semibold text-high">Unexplained {unexplained}</span>
            </>
          )}
          {notes.length > 0 && `${ticks.total > 0 || unexplained > 0 ? " · " : ""}${notes.join(" · ")}`}
        </span>
      </button>
    </li>
  );
}

/**
 * The Files view's tree (brief §4.5): Code first, then each Not analysed group with its path-rule badge; per file
 * its GitHub Viewed mark, changed lines, and the step ticks of the Flows view (read-only).
 */
export function FileTree({
  files,
  tour,
  ticked,
  current,
  viewedStatus,
  filter,
  onFilter,
  onSelect,
}: FileTreeProps) {
  const id = useId();
  const needle = filter.trim().toLowerCase();
  const shown = needle ? files.filter((f) => f.path.toLowerCase().includes(needle)) : files;
  const { viewed, total } = viewedCount(files);
  return (
    <div className="grid gap-2">
      {viewedStatus === "not_checked" ? (
        <p className="m-0 text-[12px] text-muted">
          Viewed state not read from GitHub (GitHub did not answer); nothing is assumed.
        </p>
      ) : (
        <Tooltip tip="Files you marked Viewed on GitHub for this pull request, out of all its files. Separate from step ticks.">
          <p tabIndex={0} className="m-0 cursor-help text-[12.5px]">
            Viewed on GitHub: <b>{viewed}</b> / {total} files
            {viewedStatus === "partial" && " (only the first files were read)"}
          </p>
        </Tooltip>
      )}
      <label htmlFor={`${id}-f`} className="sr-only">
        Filter files
      </label>
      <input
        id={`${id}-f`}
        type="search"
        value={filter}
        placeholder="Filter files"
        onChange={(event) => onFilter(event.target.value)}
        className="min-h-7 w-full rounded-md border border-border bg-surface px-2 text-[12.5px]"
      />
      {shown.length === 0 && <p className="m-0 text-[12.5px] text-muted">No file matches the filter.</p>}
      {treeGroups(shown).map((group) => {
        const rule = group.files[0]?.group;
        let lastDir: string | null = null;
        return (
          <section key={group.key} aria-labelledby={`${id}-${group.key}`}>
            <h3
              id={`${id}-${group.key}`}
              className="m-0 mb-1 flex items-center gap-1.5 text-[10.5px] font-bold tracking-wide text-muted uppercase"
            >
              {group.key === "code" ? "Code" : NOT_ANALYSED_LABEL[group.key]} · {group.files.length}
              {rule?.kind === "not_analysed" && <ProvenanceBadge badge={rule.badge} />}
            </h3>
            <ul className="m-0 grid list-none gap-0.5 p-0">
              {group.files.map((file) => {
                const at = dir(file.path);
                const heading = at !== lastDir;
                lastDir = at;
                return [
                  heading && (
                    <li key={`d:${at}`} aria-hidden className="pt-1 pl-2 font-mono text-[11px] text-muted">
                      {at}
                    </li>
                  ),
                  <Row
                    key={file.path}
                    file={file}
                    current={file.path === current}
                    tour={tour}
                    ticked={ticked}
                    onSelect={onSelect}
                  />,
                ];
              })}
            </ul>
          </section>
        );
      })}
      <p className="mt-3 text-xs text-muted">
        <Kbd>j</Kbd> <Kbd>k</Kbd> hunk · <Kbd>n</Kbd> <Kbd>p</Kbd> file · <Kbd>v</Kbd> viewed · <Kbd>s</Kbd>{" "}
        Flows · <Kbd>?</Kbd> help
      </p>
    </div>
  );
}
