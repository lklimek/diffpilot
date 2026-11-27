import { useId, useState } from "react";
import type { GithubThread } from "../../api/generated/types";
import { commentText } from "../../lib/commentText";
import { inApp } from "../inApp";
import { CodeText } from "../section/CodeText";
import { Tooltip } from "../tooltip/Tooltip";

export interface GithubThreadActions {
  loadBody: (id: string) => Promise<{ body: string } | { error: string }>;
  href: (id: string) => string;
  navigate: (href: string) => void;
}

/** Displays the entire original body. Code and spacing retain their formatting. */
export function GithubThreadBody({ text }: { text: string }) {
  return (
    <span className="block whitespace-pre-wrap [overflow-wrap:anywhere]">
      <CodeText text={text} linkPaths={false} />
    </span>
  );
}

/** The server marks a clipped excerpt with a single ellipsis after 200 Unicode characters. */
export function GithubThreadPreview({
  thread,
  actions,
  compact,
}: {
  thread: GithubThread;
  actions?: GithubThreadActions;
  compact?: boolean;
}) {
  const target = useId();
  const [expanded, setExpanded] = useState(false);
  const [body, setBody] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const clipped = thread.excerpt.endsWith("…") && Array.from(thread.excerpt).length === 201;
  const previewLayout = compact && clipped ? "line-clamp-4" : "block";
  const previewText = commentText(thread.excerpt);
  const preview = <CodeText text={previewText || (clipped ? "Show the full comment" : "")} linkPaths={false} />;
  if (!actions || !clipped)
    return <span className={`${previewLayout} whitespace-pre-wrap`}>{preview}</span>;

  const toggle = async () => {
    setExpanded(!expanded);
    if (expanded || loading || body !== null) return;
    setError(null);
    setLoading(true);
    try {
      const result = await actions.loadBody(thread.id);
      if ("body" in result) setBody(result.body);
      else setError(result.error);
    } finally {
      setLoading(false);
    }
  };
  return (
    <span className="block">
      <Tooltip tip={expanded ? "Collapse the comment" : "Show the full comment"}>
        <button
          type="button"
          aria-expanded={expanded}
          aria-controls={target}
          onClick={() => void toggle()}
          className={`${previewLayout} cursor-pointer rounded text-left whitespace-pre-wrap text-accent hover:underline [overflow-wrap:anywhere]`}
        >
          {preview}
        </button>
      </Tooltip>
      <span id={target} hidden={!expanded} className="mt-1">
        {loading && <span role="status">Reading the GitHub comment…</span>}
        {error && <span role="alert">Could not read the GitHub comment. {error}</span>}
        {body !== null && <GithubThreadBody text={body} />}
      </span>
    </span>
  );
}

/** Opens the exact discussion in the PR's Comments view. */
export function OpenThreadInComments({ id, actions }: { id: string; actions: GithubThreadActions }) {
  const href = actions.href(id);
  return (
    <a
      className="text-accent underline"
      href={href}
      onClick={(event) => inApp(event, () => actions.navigate(href))}
    >
      Open in Comments
    </a>
  );
}
