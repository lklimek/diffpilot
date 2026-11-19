import type { ScanNote, TodosState } from "../../../api/generated/types";
import { clockText } from "../../format/time";
import { Tooltip } from "../../tooltip/Tooltip";
import { navigateOnClick, type Navigate } from "../common/navigate";
import { AsideSection } from "../side-panel/AsideSection";

interface ScanNotesProps {
  notes: ScanNote[];
  /** Configured repositories and owners the user's GitHub token cannot see. */
  unreachable: string[];
  todos: TodosState;
  /** In-app href of Settings, where most notes are fixed (refused accounts, unreachable repositories). */
  settingsHref?: string;
  onNavigate?: Navigate;
  now: Date;
}

/** What the scan missed or approximated (side panel; skill "What the scan missed"). */
export function ScanNotes({ notes, unreachable, todos, settingsHref, onNavigate, now }: ScanNotesProps) {
  const lines = [
    ...unreachable.map(
      (name) =>
        `Not visible to your GitHub token: ${name} — install the GitHub App there or grant it access.`,
    ),
    ...(todos.status === "not_refreshed"
      ? [
          `MemCan TODOs not refreshed (MemCan unreachable${todos.failed_at ? ` at ${clockText(todos.failed_at, now)}` : ""}); the TODO rows are from an earlier scan.`,
        ]
      : []),
    ...notes.map((note) => note.text),
  ];
  return (
    <AsideSection
      title="What the scan missed"
      defaultOpen={unreachable.length > 0 || todos.status === "not_refreshed"}
      summary={
        <>
          <b>{lines.length}</b> {lines.length === 1 ? "note" : "notes"}
        </>
      }
    >
      {lines.length === 0 ? (
        <p className="text-xs text-muted">No notes from this scan.</p>
      ) : (
        <ol className="m-0 pl-[18px] text-xs">
          {lines.map((line, i) => (
            <li key={i} className="mb-1.5">
              {line}
            </li>
          ))}
        </ol>
      )}
      {lines.length > 0 && settingsHref && (
        <Tooltip tip="Opens Settings → Dashboard, where accounts, repositories and the owners to list when GitHub search refuses an account are set.">
          <a
            href={settingsHref}
            onClick={navigateOnClick(settingsHref, onNavigate)}
            className="text-xs text-accent underline"
          >
            Open Settings
          </a>
        </Tooltip>
      )}
    </AsideSection>
  );
}
