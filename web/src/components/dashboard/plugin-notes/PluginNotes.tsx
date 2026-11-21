import type { PluginNote } from "../../../api/generated/types";
import { clockText } from "../../format/time";
import { OpenExternal } from "../common/OpenExternal";

interface PluginNotesProps {
  /** The plug-ins' statements about one pull request (absent: none). */
  notes: PluginNote[] | undefined;
  /** Display id of the pull request, for the evidence links' names. */
  itemId: string;
  /** Also list the points beyond the headline (the expanded row). */
  full?: boolean;
  now: Date;
}

const factsText = (note: PluginNote) => note.facts.map((fact) => `${fact.label}: ${fact.value}`).join(" · ");

/**
 * External statements of dashboard plug-ins about a pull request (ADR-022): each attributed to its source
 * ("PR Hygiene: …"), plain text, with a link to its evidence and the source's data time; never a diffpilot tag or
 * verdict. `full` lists the points beyond the headline.
 */
export function PluginNotes({ notes, itemId, full = false, now }: PluginNotesProps) {
  if (!notes || notes.length === 0) return null;
  return (
    <div className="mt-1 grid gap-1">
      {notes.map((note) => {
        const details = note.details.filter((detail) => detail !== note.summary);
        return (
          <div key={note.plugin} data-plugin-note={note.plugin} className="text-[12.5px]">
            <p className="m-0 [overflow-wrap:anywhere]">
              <span className="font-semibold">{note.source}:</span>{" "}
              {note.summary ?? <span className="text-muted italic">no next step stated</span>}{" "}
              <OpenExternal href={note.url} label={`${note.source} data for ${itemId}`} />
            </p>
            <p className="m-0 text-xs text-muted [overflow-wrap:anywhere]">
              {note.facts.length > 0 && `${factsText(note)} · `}
              {`as of ${clockText(note.as_of, now)}`}
            </p>
            {full && details.length > 0 && (
              <ul aria-label={`${note.source}: further points`} className="m-0 mt-0.5 list-disc pl-5 text-xs">
                {details.map((detail, i) => (
                  <li key={`${i}-${detail}`} className="[overflow-wrap:anywhere]">
                    {detail}
                  </li>
                ))}
              </ul>
            )}
          </div>
        );
      })}
    </div>
  );
}
