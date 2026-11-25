import { useNavigate } from "react-router";
import { ApiError } from "../../api/client";
import { useCarryOvers } from "../../api/review-loop";
import { useNow } from "../../app/now";
import { CarryOvers } from "../../components/comments/CarryOvers";
import { StatusMessage } from "../../components/status/StatusMessage";
import { ALL_CARRY_OVERS_PATH } from "../pr/paths";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

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
