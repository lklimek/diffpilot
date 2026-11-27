import { Link, Navigate, Outlet, useLocation, useParams } from "react-router";
import { ApiError, errorText } from "../../api/client";
import { routes } from "../../api/generated/routes";
import { useAnalysisOnce } from "../../api/review-loop";
import { StatusMessage } from "../../components/status/StatusMessage";
import { MY_WORK_PATH } from "../shell/paths";
import { analysisPath, prOfParams, prPath, samePr } from "./paths";

function AdmissionStatus({ error }: { error?: unknown }) {
  return (
    <main id="main" className="flex min-h-screen flex-col items-center justify-center gap-4 px-8">
      {error ? (
        <>
          <StatusMessage kind="error">{errorText(error)}</StatusMessage>
          {error instanceof ApiError && error.code === "unauthenticated" ? (
            <a href={routes.authLogin.path}>Sign in again</a>
          ) : (
            <Link to={MY_WORK_PATH}>My work</Link>
          )}
        </>
      ) : (
        <StatusMessage kind="loading">Connecting…</StatusMessage>
      )}
    </main>
  );
}

/** Admit only the pull request from the authorized analysis lookup. */
export function AnalysisAdmission() {
  const params = useParams();
  const analysis = useAnalysisOnce(params.analysisId ?? "");
  if (analysis.isError || !analysis.data) return <AdmissionStatus error={analysis.error} />;
  if (params.number !== String(analysis.data.pr.number) || !samePr(prOfParams(params), analysis.data.pr)) {
    return (
      <AdmissionStatus
        error={new ApiError(404, "not_found", "This analysis does not belong to this pull request.")}
      />
    );
  }
  return <Outlet context={analysis.data.pr} />;
}

/** Redirect old bookmarks using the saved PR, carrying the original destination and location. */
export function LegacyAnalysisRedirect({ view }: { view?: "files" | "deps" | "progress" }) {
  const { analysisId = "" } = useParams();
  const { search, hash } = useLocation();
  const analysis = useAnalysisOnce(analysisId);
  if (analysis.isError || !analysis.data) return <AdmissionStatus error={analysis.error} />;
  const pr = analysis.data.pr;
  if (view === "progress") {
    return (
      <Navigate
        to={{ pathname: prPath(pr), search, hash }}
        state={{ progressId: analysis.data.id }}
        replace
      />
    );
  }
  return <Navigate to={{ pathname: analysisPath(pr, analysis.data.id, view), search, hash }} replace />;
}
