// V2–V5 of the Comments view, inside the Comments layout (CommentsRoute).
import { useState } from "react";
import { useNavigate } from "react-router";
import { ApiError } from "../../api/client";
import { useCarryOvers, useCommentLog, useDecideBatch } from "../../api/review-loop";
import { CarryOvers } from "../../components/comments/CarryOvers";
import { CommentsTable } from "../../components/comments/CommentsTable";
import { DecisionLog } from "../../components/comments/DecisionLog";
import { RoundsTable } from "../../components/comments/RoundsTable";
import { StatusMessage } from "../../components/status/StatusMessage";
import { useAskChat } from "../review/askChat";
import { ALL_CARRY_OVERS_PATH, commentsPath, itemPath } from "../pr/paths";
import { useComments } from "./CommentsRoute";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

/** V2 Table page (RQ21). */
export function TableScreen() {
  const { pr, overview, items, askAnswers, askAvailable, now } = useComments();
  const navigate = useNavigate();
  const chat = useAskChat();
  const batch = useDecideBatch(pr);
  const [done, setDone] = useState("");
  return (
    <>
      <CommentsTable
        key={items.map((i) => `${i.id}:${i.revision}`).join(",")}
        prNumber={pr.number}
        purpose={overview.purpose}
        items={items}
        askAnswers={askAnswers}
        itemHref={(item) => itemPath(pr, item.no)}
        onNavigate={(href) => void navigate(href)}
        onOpenAsk={
          askAvailable
            ? (item) => chat.open({ anchor: { kind: "comment_item", item: item.id, no: item.no } })
            : undefined
        }
        onSubmit={(rows) =>
          batch.mutate(
            { via: "table", rows },
            { onSuccess: (stored) => setDone(`${stored.length} decisions recorded, one per row.`) },
          )
        }
        busy={batch.isPending}
        error={batch.isError ? errorText(batch.error) : null}
        now={now}
      />
      <p role="status" className="mt-2 text-sm">
        {done}
      </p>
    </>
  );
}

/** V3 Decision log and execution. */
export function LogScreen() {
  const { pr, login, now } = useComments();
  const log = useCommentLog(pr);
  if (log.isError) return <StatusMessage kind="error">{errorText(log.error)}</StatusMessage>;
  if (!log.data) return <StatusMessage kind="loading">Loading the decision log…</StatusMessage>;
  return <DecisionLog log={log.data} now={now} storageKey={`${login}:comments-log`} />;
}

/** V4 Rounds. */
export function RoundsScreen() {
  const { pr, overview } = useComments();
  return <RoundsTable rounds={overview.rounds} prNumber={pr.number} />;
}

/** V5 Carry-overs of this pull request. */
export function CarryOversScreen() {
  const { pr, now } = useComments();
  const navigate = useNavigate();
  const page = useCarryOvers(`${pr.repo.owner}/${pr.repo.name}/${pr.number}`);
  if (page.isError) return <StatusMessage kind="error">{errorText(page.error)}</StatusMessage>;
  if (!page.data) return <StatusMessage kind="loading">Loading the carry-overs…</StatusMessage>;
  return (
    <CarryOvers
      page={page.data}
      scope="pr"
      prHref={commentsPath(pr, "carry-overs")}
      allHref={ALL_CARRY_OVERS_PATH}
      onNavigate={(href) => void navigate(href)}
      itemHref={(no) => itemPath(pr, no)}
      now={now}
    />
  );
}
