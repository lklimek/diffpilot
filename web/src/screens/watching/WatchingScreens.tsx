// Shell pages of the review loop: watched PRs (V6) and carry-overs of all pull requests (V5).
import { useNavigate } from "react-router";
import { ApiError } from "../../api/client";
import { useAddWatch, useCarryOvers, useUnwatch, useWatches } from "../../api/review-loop";
import { useNow } from "../../app/now";
import { CarryOvers } from "../../components/comments/CarryOvers";
import { WatchedList } from "../../components/comments/WatchedList";
import { WatchPrForm } from "../../components/comments/WatchPrForm";
import { StatusMessage } from "../../components/status/StatusMessage";
import { ALL_CARRY_OVERS_PATH, prPath } from "../pr/paths";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

/** V6 `/watching`. */
export function WatchingScreen() {
  const list = useWatches();
  const unwatch = useUnwatch();
  const watch = useAddWatch();
  const navigate = useNavigate();
  const now = new Date(useNow());
  return (
    <div className="grid gap-3">
      <h1 className="m-0 text-xl font-bold">Watched pull requests</h1>
      {list.data && <WatchPrForm list={list.data} onWatch={watch.mutateAsync} />}
      {list.isError && <StatusMessage kind="error">{errorText(list.error)}</StatusMessage>}
      {list.isPending && <StatusMessage kind="loading">Loading the watched pull requests…</StatusMessage>}
      {unwatch.isError && <StatusMessage kind="error">{errorText(unwatch.error)}</StatusMessage>}
      {list.data && (
        <WatchedList
          list={list.data}
          prHref={prPath}
          onNavigate={(href) => void navigate(href)}
          onUnwatch={(pr) => unwatch.mutate(pr)}
          busy={unwatch.isPending}
          now={now}
        />
      )}
    </div>
  );
}

/** V5 `/comments/carry-overs`: all pull requests. */
export function AllCarryOversScreen() {
  const page = useCarryOvers(null);
  const navigate = useNavigate();
  const now = new Date(useNow());
  return (
    <div className="grid gap-3">
      <h1 className="m-0 text-xl font-bold">Carry-overs on all pull requests</h1>
      {page.isError && <StatusMessage kind="error">{errorText(page.error)}</StatusMessage>}
      {page.isPending && <StatusMessage kind="loading">Loading the carry-overs…</StatusMessage>}
      {page.data && (
        <CarryOvers
          page={page.data}
          scope="all"
          prHref={null}
          allHref={ALL_CARRY_OVERS_PATH}
          onNavigate={(href) => void navigate(href)}
          now={now}
        />
      )}
    </div>
  );
}
