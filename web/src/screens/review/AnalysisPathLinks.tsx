import { useMemo, type ReactNode } from "react";
import { useNavigate } from "react-router";
import type { PrFile, PrRef } from "../../api/generated/types";
import { useFiles, useTour } from "../../api/queries";
import { displaySide, filesHash } from "../../components/files/model";
import { githubLineUrl, type PrLinkBase } from "../../components/github/links";
import { PathLinks, pathMatcher } from "../../components/section/pathLinks";

import { analysisPath } from "../pr/paths";

export function AnalysisPathLinks({
  analysisId,
  pr,
  head,
  children,
}: {
  analysisId: string | null;
  pr: PrRef;
  head?: string | null;
  children: ReactNode;
}) {
  const tour = useTour(analysisId ?? "", analysisId !== null);
  const analysedPr = tour.data?.analysis.pr;
  const matchesHead = !!analysedPr && (head === undefined || head === analysedPr.head_sha);
  // Review screens already read Files; the Comments route needs its own read after checking the head.
  const files = useFiles(analysisId ?? "", analysisId !== null && head !== undefined && matchesHead);
  return (
    <FilePathLinks
      analysisId={analysisId}
      pr={pr}
      files={matchesHead ? files.data?.files : undefined}
      githubPr={analysedPr}
    >
      {children}
    </FilePathLinks>
  );
}

/** Resolve mentions from a single known analysis. Without its Files index they remain plain text. */
export function FilePathLinks({
  analysisId,
  pr,
  files,
  githubPr,
  children,
}: {
  analysisId: string | null;
  pr: PrRef;
  files?: readonly PrFile[];
  githubPr?: PrLinkBase;
  children: ReactNode;
}) {
  const navigate = useNavigate();
  const links = useMemo(() => {
    if (analysisId === null || !files) return null;
    const filesPath = analysisPath(pr, analysisId, "files");
    const byPath = new Map(files.map((file) => [file.path, file]));
    const match = pathMatcher([...byPath.keys()], githubPr?.repo ?? pr.repo);
    return {
      resolve: (mention: string) => {
        const at = match(mention);
        if (!at) return;
        const file = byPath.get(at.path)!;
        const side = displaySide(file);
        const line = at.line ?? 1;
        const range = at.end === undefined ? at.line : `${line}–${at.end}`;
        const location = range === undefined ? "" : ` at ${at.end === undefined ? "line" : "lines"} ${range}`;
        if (file.not_rendered && githubPr) {
          const href = githubLineUrl(
            githubPr,
            { path: at.path, side, start: line, end: at.end ?? line },
            false,
          );
          return {
            href: at.line === undefined ? href.split("#")[0]! : href,
            tip: `Open ${at.path}${location} on GitHub at the analysed commit (new tab)`,
            github: true,
          };
        }
        const target = file.not_rendered ? {} : at;
        return {
          href: `${filesPath}${filesHash({ ...target, file: at.path, side })}`,
          tip: `Open ${at.path}${file.not_rendered ? "" : location} in Files`,
        };
      },
      onNavigate: (href: string) => void navigate(href),
    };
  }, [analysisId, pr, files, githubPr, navigate]);
  return <PathLinks.Provider value={links}>{children}</PathLinks.Provider>;
}
