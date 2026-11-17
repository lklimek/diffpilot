import type { Evidence } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";
import { evidenceText } from "./format";

/** Structured citations grouped by file, disclosed below their statement. */
export function EvidenceLinks({
  evidence,
  onEvidence,
  currentFile,
  basis,
}: {
  evidence: readonly Evidence[];
  onEvidence?: (evidence: Evidence) => void;
  currentFile?: string;
  basis?: string;
}) {
  if (evidence.length === 0) return basis ? <span className="text-xs text-muted">({basis})</span> : null;
  const files = [...new Set(evidence.map((e) => e.file))];
  return (
    <details className="mt-1 text-xs text-muted">
      <summary className="cursor-pointer" title="Show the source lines supporting this statement.">
        Evidence · {evidence.length} {evidence.length === 1 ? "place" : "places"}
        {basis && ` (${basis})`}
      </summary>
      {files.map((file) => (
        <div key={file} className="flex flex-wrap items-center gap-1">
          {file !== currentFile && (
            <Tooltip tip={file}>
              <span tabIndex={0} className="font-mono">
                {file.split("/").at(-1)}
              </span>
            </Tooltip>
          )}
          {evidence
            .filter((e) => e.file === file)
            .map((item, i) => {
              const label = `:${item.line}${item.side === "base" ? " (base)" : ""}`;
              return onEvidence ? (
                <Tooltip key={i} tip={`${evidenceText(item)} — show this line in the review.`}>
                  <button
                    type="button"
                    aria-label={`Show ${evidenceText(item)}`}
                    className="cursor-pointer font-mono text-xs text-accent underline"
                    onClick={() => onEvidence(item)}
                  >
                    {label}
                  </button>
                </Tooltip>
              ) : (
                <code key={i} title={file}>
                  {label}
                </code>
              );
            })}
        </div>
      ))}
    </details>
  );
}
