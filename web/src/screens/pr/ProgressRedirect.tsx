import { Link, Navigate, useLocation, useParams } from "react-router";
import { ApiError } from "../../api/client";
import { useAnalysisOnce } from "../../api/review-loop";
import { StatusMessage } from "../../components/status/StatusMessage";
import { MY_WORK_PATH } from "../shell/paths";
import { prPath } from "./paths";

/** `/analyses/:id/progress` (old URL): the PR page of the analysis's pull request, with the same query. */
export function ProgressRedirect() {
  const { analysisId = "" } = useParams();
  const { search } = useLocation();
  const analysis = useAnalysisOnce(analysisId);
  if (analysis.data)
    return (
      <Navigate
        to={{ pathname: prPath(analysis.data.pr), search }}
        state={{ progressId: analysisId }}
        replace
      />
    );
  return (
    <main id="main" className="flex min-h-screen flex-col items-center justify-center gap-4 px-8">
      {analysis.isError ? (
        <>
          <StatusMessage kind="error">
            {analysis.error instanceof ApiError ? analysis.error.message : "Something went wrong. Try again."}
          </StatusMessage>
          <Link to={MY_WORK_PATH}>My work</Link>
        </>
      ) : (
        <StatusMessage kind="loading">Connecting…</StatusMessage>
      )}
    </main>
  );
}
