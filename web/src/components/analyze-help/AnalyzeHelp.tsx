import { useId } from "react";

/** Aside of Home: what happens after Start analysis, and where the settings are. */
export function AnalyzeHelp({
  llmHref,
  githubSyncHref,
  repositoriesHref,
}: {
  llmHref: string;
  githubSyncHref: string;
  repositoriesHref: string;
}) {
  const titleId = useId();
  return (
    <aside
      aria-labelledby={titleId}
      className="grid gap-2 rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <h2 id={titleId} className="text-base font-semibold">
        What happens after Start analysis
      </h2>
      <ol className="list-decimal pl-5 text-[13px] [&>li]:mb-1.5">
        <li>Check looks the PR up; nothing starts until you press Start analysis.</li>
        <li>The agent reads the PR, plans the flows and writes the PR context; then the review opens.</li>
        <li>Flows are analysed when you choose them, or all of them with Analyse all remaining flows.</li>
        <li>Comments you write go straight into your GitHub pending review; you submit it on GitHub.</li>
      </ol>
      <p className="text-xs text-muted">
        An existing analysis of the same PR is offered instead of a new one.
      </p>
      <p className="text-xs text-muted">
        Settings pages for this review include <a href={llmHref}>LLM &amp; usage</a>,{" "}
        <a href={githubSyncHref}>GitHub sync</a> and <a href={repositoriesHref}>Repositories</a>.
      </p>
    </aside>
  );
}
