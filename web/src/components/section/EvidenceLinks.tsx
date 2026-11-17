import { Fragment } from "react";
import type { Evidence } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";
import { evidenceText } from "./format";

/** Structured evidence destinations; without an action they remain plain text. */
export function EvidenceLinks({
  evidence,
  onEvidence,
}: {
  evidence: readonly Evidence[];
  onEvidence?: (evidence: Evidence) => void;
}) {
  return (
    <>
      {evidence.map((item, i) => (
        <Fragment key={`${item.file}:${item.side}:${item.line}:${i}`}>
          {i > 0 && ", "}
          {onEvidence ? (
            <Tooltip tip="Shows this line in the review, on its cited commit side.">
              <button
                type="button"
                aria-label={`Show ${evidenceText(item)}`}
                className="cursor-pointer break-all font-mono text-xs text-accent underline"
                onClick={() => onEvidence(item)}
              >
                {evidenceText(item)}
              </button>
            </Tooltip>
          ) : (
            <code>{evidenceText(item)}</code>
          )}
        </Fragment>
      ))}
    </>
  );
}
