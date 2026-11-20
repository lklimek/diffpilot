import type { CrateChanges, NotesState } from "../../api/generated/types";
import { CodeText } from "../section/CodeText";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { LineLinkTo } from "./LineLinkTo";
import { count, type DepsLinks } from "./model";

const NONE: Record<NotesState, string> = {
  produced: "The LLM wrote no notes for this crate.",
  pending: "LLM notes are not written yet.",
  not_produced: "LLM notes were not produced for this analysis.",
};

/** The LLM's keyword notes on a changed crate: labelled LLM, at most three, the rest counted. */
export function CrateNotes({
  changes,
  notes,
  links,
}: {
  changes: CrateChanges;
  /** Whether the notes of the analysis exist. */
  notes: NotesState;
  links: DepsLinks;
}) {
  if (changes.notes.length === 0) return <p className="m-0 text-muted">{NONE[notes]}</p>;
  return (
    <div className="grid gap-1">
      <ul className="m-0 grid list-none gap-1 p-0">
        {changes.notes.map((note, index) => (
          <li key={index} className="flex flex-wrap items-baseline gap-x-1.5">
            <ProvenanceTag source="llm" />
            <span className="[overflow-wrap:anywhere]">
              <CodeText text={note.text} />
            </span>
            {note.lines.map((r) => (
              <LineLinkTo
                key={`${r.file}:${r.side}:${r.range.start_line}`}
                target={{ file: r.file, side: r.side, line: r.range.start_line, end: r.range.end_line }}
                links={links}
              />
            ))}
          </li>
        ))}
      </ul>
      {changes.notes_hidden > 0 && (
        <p className="m-0 text-muted">+{count(changes.notes_hidden, "more note")}, not listed.</p>
      )}
    </div>
  );
}
