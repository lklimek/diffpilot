import type { NoteElsewhere, NoteSource } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

const GLYPH: Record<NoteSource, string> = {
  flows: "⚑",
  files: "✎",
  dependencies: "⇄",
  pr_hygiene: "☑",
  thepastaclaw: "◔",
};

/** Icons for notes about this code elsewhere (US26): each named and tipped, opening its place in a new tab. */
export function NotesElsewhere({ notes }: { notes: NoteElsewhere[] }) {
  if (notes.length === 0) return <span className="text-xs text-muted">none</span>;
  return (
    <ul aria-label="Notes about this code elsewhere" className="m-0 flex list-none flex-wrap gap-1 p-0">
      {notes.map((note) => (
        <li key={`${note.source}-${note.url}`}>
          <Tooltip tip={`${note.label} (new tab)`}>
            <a
              href={note.url}
              target="_blank"
              rel="noopener noreferrer"
              aria-label={`${note.label} (new tab)`}
              data-note={note.source}
              className="inline-flex h-[26px] min-w-7 items-center justify-center gap-0.5 rounded-md border border-border bg-surface px-1.5 text-sm text-text no-underline hover:border-accent"
            >
              <span aria-hidden="true">{GLYPH[note.source] ?? "•"}</span>
              {note.count !== null && (
                <span aria-hidden="true" className="text-[10px] font-bold text-muted">
                  {note.count}
                </span>
              )}
            </a>
          </Tooltip>
        </li>
      ))}
    </ul>
  );
}
