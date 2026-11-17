import { useId } from "react";

/** Aside of Home: what happens after Analyze, and where the settings are. */
export function AnalyzeHelp({ settingsHref }: { settingsHref: string }) {
  const titleId = useId();
  return (
    <aside
      aria-labelledby={titleId}
      className="grid gap-2 rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <h2 id={titleId} className="text-base font-semibold">
        What happens after Analyze
      </h2>
      <ol className="list-decimal pl-5 text-[13px] [&>li]:mb-1.5">
        <li>The agent reads the PR and plans the flows.</li>
        <li>When the analysis has finished, the review opens.</li>
        <li>Comments you write go straight into your GitHub pending review; you submit it on GitHub.</li>
      </ol>
      <p className="text-xs text-muted">
        An existing analysis of the same PR is resumed instead of started again.
      </p>
      <p className="text-xs text-muted">
        <a href={settingsHref}>Settings</a>: LLM status, language of LLM texts, GitHub App and repository
        rules.
      </p>
    </aside>
  );
}
