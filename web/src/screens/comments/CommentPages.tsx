// V2–V5 of the Comments view, inside the Comments layout (CommentsRoute).
import { useState } from "react";
import { useNavigate } from "react-router";
import { errorText } from "../../api/client";
import {
  useCarryOvers,
  useCommentLog,
  useDecideBatch,
  useItemAction,
  useUndoDecision,
} from "../../api/review-loop";
import { CarryOvers } from "../../components/comments/CarryOvers";
import { CommentsTable } from "../../components/comments/CommentsTable";
import { DecisionLog } from "../../components/comments/DecisionLog";
import { RoundsTable } from "../../components/comments/RoundsTable";
import { CHOICE_LABEL } from "../../components/comments/decisions";
import { StatusMessage } from "../../components/status/StatusMessage";
import { useAskChat } from "../review/askChat";
import { ALL_CARRY_OVERS_PATH, commentsPath, itemPath } from "../pr/paths";
import { useComments } from "./CommentsRoute";

/** V2 Table page (RQ21). */
export function TableScreen() {
  const { pr, overview, items, askAnswers, now } = useComments();
  const navigate = useNavigate();
  const chat = useAskChat();
  const batch = useDecideBatch(pr);
  const [done, setDone] = useState("");
  // A recorded batch starts the rows afresh; a read of GitHub keeps what the reader typed.
  const [recorded, setRecorded] = useState(0);
  return (
    <>
      <CommentsTable
        key={recorded}
        prNumber={pr.number}
        purpose={overview.purpose}
        prHead={overview.head}
        items={items}
        askAnswers={askAnswers}
        itemHref={(item) => itemPath(pr, item.no)}
        onNavigate={(href) => void navigate(href)}
        onOpenAsk={(item) => chat.open({ anchor: { kind: "comment_item", item: item.id } })}
        onSubmit={(rows) =>
          batch.mutate(
            { via: "table", rows },
            {
              onSuccess: (stored) => {
                const marks = stored.filter((entry) => entry.kind === "not_a_review_point");
                const decisions = stored.length - marks.length;
                const messages = [];
                if (marks.length === 1)
                  messages.push(`C${marks[0]!.item_no}: ${CHOICE_LABEL.not_a_review_point}.`);
                else if (marks.length > 1)
                  messages.push(`${marks.length} items marked “${CHOICE_LABEL.not_a_review_point}”.`);
                if (decisions) {
                  messages.push(`${decisions} ${decisions === 1 ? "decision" : "decisions"} recorded.`);
                  setRecorded((n) => n + 1);
                }
                setDone(messages.join(" "));
              },
            },
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
  const { pr, login, now, items } = useComments();
  const log = useCommentLog(pr);
  const undo = useUndoDecision(pr);
  if (log.isError) return <StatusMessage kind="error">{errorText(log.error)}</StatusMessage>;
  if (!log.data) return <StatusMessage kind="loading">Loading the decision log…</StatusMessage>;
  return (
    <DecisionLog
      log={log.data}
      githubDecisions={items.filter((item) => item.imported_decision || item.state === "outside")}
      pr={pr}
      origin={window.location.origin}
      now={now}
      storageKey={`${login}:comments-log`}
      undoBusy={undo.isPending}
      undoError={undo.isError ? errorText(undo.error) : null}
      onUndoMark={(mark) => undo.mutate({ no: mark.item_no, decision: mark.id, mark: true })}
    />
  );
}

/** V4 Rounds. */
export function RoundsScreen() {
  const { pr, overview } = useComments();
  return <RoundsTable rounds={overview.rounds} prNumber={pr.number} />;
}

/** V5 Carry-overs of this pull request; Reopen asks a carried item again (RC13) and opens it. */
export function CarryOversScreen() {
  const { pr, items, now } = useComments();
  const navigate = useNavigate();
  const page = useCarryOvers(`${pr.repo.owner}/${pr.repo.name}/${pr.number}`);
  const action = useItemAction(pr);
  if (page.isError) return <StatusMessage kind="error">{errorText(page.error)}</StatusMessage>;
  if (!page.data) return <StatusMessage kind="loading">Loading the carry-overs…</StatusMessage>;
  return (
    <>
      <CarryOvers
        page={page.data}
        scope="pr"
        prHref={commentsPath(pr, "carry-overs")}
        allHref={ALL_CARRY_OVERS_PATH}
        onNavigate={(href) => void navigate(href)}
        itemHref={(no) => itemPath(pr, no)}
        onReopen={(event) => {
          const item = items.find((i) => i.no === event.item_no);
          if (!item || action.isPending) return;
          action.mutate(
            { action: "reopen", no: item.no, revision: item.revision },
            { onSuccess: () => void navigate(itemPath(pr, item.no)) },
          );
        }}
        onShowOlder={() => void page.fetchNextPage()}
        loadingOlder={page.isFetchingNextPage}
        now={now}
      />
      {action.isError && <StatusMessage kind="error">{errorText(action.error)}</StatusMessage>}
    </>
  );
}
