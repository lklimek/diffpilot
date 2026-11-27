import type { AgentAnnotation, CommentItem } from "../../api/generated/types";
import { dateTimeText } from "../format/time";
import { GithubLink } from "../inspector/text";
import type { ProvenanceSource } from "../section/ProvenanceTag";

/** The primary comment and its supplemental sources are labeled together. */
export function commentProvenance(item: CommentItem): ProvenanceSource | null {
  if (item.origin === "api") return "api";
  if (item.origin === "diffpilot") return null;
  return item.agent_annotations?.length ? "github_agent" : "github";
}

/** Each annotation keeps its own author, text and exact GitHub target. */
export function AgentAnnotations({
  annotations,
  showTarget = true,
}: {
  annotations?: readonly AgentAnnotation[];
  showTarget?: boolean;
}) {
  if (!annotations?.length) return null;
  return (
    <ul aria-label="Agent annotations" className="m-0 mt-2 grid list-none gap-2 p-0">
      {annotations.map((annotation) => (
        <li
          id={`agent-comment-${annotation.id}`}
          key={annotation.id}
          className="rounded-md border border-llm bg-llm-bg px-3 py-2 text-[13px] wrap-anywhere"
        >
          <div className="flex flex-wrap items-baseline gap-x-2 text-xs">
            <strong>{annotation.agent} · agent</strong>
            <time dateTime={annotation.created_at} className="text-muted">
              {dateTimeText(annotation.created_at)}
            </time>
            {showTarget && (
              <GithubLink url={annotation.target_url} tip="Open the exact annotated comment on GitHub">
                Source comment
              </GithubLink>
            )}
          </div>
          <p className="m-0 mt-1 whitespace-pre-wrap">{annotation.body}</p>
        </li>
      ))}
    </ul>
  );
}
