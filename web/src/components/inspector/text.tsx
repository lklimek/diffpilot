import type { ReactNode } from "react";
import type { Basis, Evidence } from "../../api/generated/types";
import { GithubMark } from "../github/GithubMark";
import { isGithubUrl } from "../github/links";
import { EvidenceLinks } from "../section/EvidenceLinks";
import { Tooltip } from "../tooltip/Tooltip";

export { isGithubUrl };

/** Link to GitHub in a new tab; plain text when the URL is not a GitHub URL. */
export function GithubLink({ url, tip, children }: { url: string; tip: string; children: ReactNode }) {
  if (!isGithubUrl(url)) return <span className="font-mono text-xs">{children}</span>;
  return (
    <Tooltip tip={tip}>
      <a href={url} target="_blank" rel="noopener noreferrer" className="font-mono text-xs">
        {children}
        <GithubMark />
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
