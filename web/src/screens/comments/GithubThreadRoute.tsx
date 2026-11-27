import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { Link, Navigate, useLocation } from "react-router";
import { api, ApiError, errorText } from "../../api/client";
import type { PrRef } from "../../api/generated/types";
import { useMe } from "../../api/queries";
import { usePrTabSubject } from "../../app/tabStatus";
import { CommentConversation } from "../../components/comments/CommentConversation";
import { prGithubUrl } from "../../components/github/links";
import { ProvenanceTag } from "../../components/section/ProvenanceTag";
import { StatusMessage } from "../../components/status/StatusMessage";
import { commentsPath, itemPath } from "../pr/paths";
import { ReviewTopBar } from "../shell/ReviewTopBar";

/** An exact thread link reaches this layout before any Comments visit or assessment work. */
export function GithubThreadRoute({ pr, id }: { pr: PrRef; id: string }) {
  const me = useMe();
  const login = me.data?.user?.login ?? null;
  const thread = useQuery({
    queryKey: ["github-thread", login, pr.repo.owner, pr.repo.name, pr.number, id],
    queryFn: () => api.getGithubThread(pr, id),
    enabled: login !== null,
    retry: false,
  });
  usePrTabSubject(pr);
  const detail = thread.data;
  const { hash } = useLocation();
  const linkedAnnotation = hash.startsWith("#agent-comment-");
  useEffect(() => {
    if (linkedAnnotation && detail)
      document.getElementById(hash.slice(1))?.scrollIntoView?.({ block: "nearest" });
  }, [hash, linkedAnnotation, detail]);
  if (detail?.item_no != null && !linkedAnnotation)
    return <Navigate replace to={itemPath(pr, detail.item_no)} />;
  const failure =
    thread.error instanceof ApiError && thread.error.status === 404
      ? "This GitHub thread was not found on this pull request."
      : thread.error
        ? `Could not read this GitHub thread. ${errorText(thread.error)}`
        : me.isError || (me.isSuccess && !login)
          ? "Sign in with GitHub to read this conversation."
          : null;

  return (
    <div className="flex min-h-screen flex-col">
      <ReviewTopBar
        login={login}
        pr={{ ...pr, title: detail?.title ?? "", html_url: prGithubUrl(pr), head_sha: "" }}
      />
      <main id="main" className="mx-auto grid w-full max-w-3xl gap-3 px-4 py-4">
        <Link to={commentsPath(pr)} className="justify-self-start text-sm text-accent underline">
          ← Comments
        </Link>
        {failure ? (
          <StatusMessage kind="error">{failure}</StatusMessage>
        ) : !detail ? (
          <StatusMessage kind="loading">Reading the GitHub thread…</StatusMessage>
        ) : (
          <>
            <CommentConversation
              heading={
                detail.pr_level ? (
                  "Pull request comment"
                ) : (
                  <>
                    Review thread on <code className="wrap-anywhere text-base">{detail.path}</code>
                  </>
                )
              }
              source={<ProvenanceTag source={detail.agent_annotations?.length ? "github_agent" : "github"} />}
              comments={detail.comments}
              annotations={detail.agent_annotations}
            />
            {detail.item_no != null && (
              <Link
                to={itemPath(pr, detail.item_no)}
                className="justify-self-start text-sm text-accent underline"
              >
                Open review item C{detail.item_no}
              </Link>
            )}
            {!detail.complete && (
              <StatusMessage kind="empty">
                GitHub has more replies. Open the thread on GitHub to read them.
              </StatusMessage>
            )}
          </>
        )}
      </main>
    </div>
  );
}
