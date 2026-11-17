import type { ReactNode } from "react";
import type { Basis, Evidence } from "../../api/generated/types";
import { EvidenceLinks } from "../section/EvidenceLinks";
import { Tooltip } from "../tooltip/Tooltip";

// Only server-built GitHub links become links (threat model T12); anything else stays text.
const isGithubUrl = (url: string) => url.startsWith("https://github.com/");

/** Link to GitHub in a new tab; plain text when the URL is not a GitHub URL. */
export function GithubLink({ url, tip, children }: { url: string; tip: string; children: ReactNode }) {
  if (!isGithubUrl(url)) return <span className="font-mono text-xs">{children}</span>;
  return (
    <Tooltip tip={tip}>
      <a href={url} target="_blank" rel="noopener noreferrer" className="font-mono text-xs">
        {children} ↗
      </a>
    </Tooltip>
  );
}

/** Evidence remains attached to the statement, disclosed on demand. */
export function BasisNote({
  basis,
  evidence,
  onEvidence,
  currentFile,
}: {
  basis: Basis;
  evidence: readonly Evidence[];
  onEvidence?: (e: Evidence) => void;
  currentFile?: string;
}) {
  return (
    <EvidenceLinks basis={basis} evidence={evidence} onEvidence={onEvidence} currentFile={currentFile} />
  );
}

/** A muted sentence for an empty section. */
export function Empty({ children }: { children: ReactNode }) {
  return <p className="text-muted">{children}</p>;
}
