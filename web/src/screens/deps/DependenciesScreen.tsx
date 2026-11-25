import { useRef, useState, type ReactNode } from "react";
import { Navigate, useNavigate, useParams } from "react-router";
import { ApiError } from "../../api/client";
import { routes } from "../../api/generated/routes";
import type { AnalysisId, FilesResponse, Tour } from "../../api/generated/types";
import { useDependencies, useFiles, useMe, useTour } from "../../api/queries";
import { buttonClass } from "../../components/button/button";
import { DependencyNotices } from "../../components/deps/DependencyNotices";
import { DependencyTree } from "../../components/deps/DependencyTree";
import type { DepsLinks } from "../../components/deps/model";
import { filesHash } from "../../components/files/model";
import { githubLineUrl } from "../../components/github/links";
import { StatusMessage } from "../../components/status/StatusMessage";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { ReviewTopBar } from "../shell/ReviewTopBar";
import { GITHUB_SYNC_SETTINGS_PATH } from "../shell/paths";
import { errorText } from "../tour/code";
import { checksHref } from "../tour/review";
import { commentsPath } from "../pr/paths";

const openKey = (id: AnalysisId) => `diffpilot.deps.open.${id}`;

function loadOpen(id: AnalysisId): Record<string, boolean> {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(openKey(id)) ?? "{}");
    return stored && typeof stored === "object" && !Array.isArray(stored)
      ? Object.fromEntries(Object.entries(stored).filter(([, v]) => typeof v === "boolean"))
      : {};
  } catch {
    return {};
  }
}

/** `/analyses/{id}/deps` (brief 2026-11-20): the crates the pull request changes and what they depend on. */
export function DependenciesScreen() {
  const { analysisId = "" } = useParams();
  const me = useMe();
  const tour = useTour(analysisId);
  const files = useFiles(analysisId);
  const login = me.data?.user?.login ?? null;
  const notFinished = [tour.error, files.error].some(
    (e) => e instanceof ApiError && e.code === "analysis_not_finished",
  );
  if (notFinished) return <Navigate to={`/analyses/${encodeURIComponent(analysisId)}/progress`} replace />;
  const failed = [tour, files].find((q) => q.isError && !q.data);
  if (failed) {
    return (
      <Shell login={login}>
        <StatusMessage kind="error">{errorText(failed.error)}</StatusMessage>
        {failed.error instanceof ApiError && failed.error.code === "unauthenticated" ? (
          <a href={routes.authLogin.path}>Sign in again</a>
        ) : (
          <a href="/">My work</a>
        )}
      </Shell>
    );
  }
  if (!tour.data || !files.data) {
    return (
      <Shell login={login}>
        <StatusMessage kind="loading">Loading the dependencies…</StatusMessage>
      </Shell>
    );
  }
  return (
    <DependenciesView
      key={analysisId}
      analysisId={analysisId}
      tour={tour.data}
      files={files.data}
      login={login}
    />
  );
}

function Shell({ login, children }: { login: string | null; children: ReactNode }) {
  return (
    <div className="flex min-h-screen flex-col">
      <ReviewTopBar login={login} />
      <main id="main" className="grid gap-3 px-8 py-5">
        {children}
      </main>
    </div>
  );
}

function DependenciesView({
  analysisId,
  tour,
  files,
  login,
}: {
  analysisId: AnalysisId;
  tour: Tour;
  files: FilesResponse;
  login: string | null;
}) {
  const navigate = useNavigate();
  const deps = useDependencies(analysisId);
  const mainRef = useRef<HTMLElement>(null);
  const [open, setOpen] = useState(() => loadOpen(analysisId));
  const base = `/analyses/${analysisId}`;
  const pr = tour.analysis.pr;
  const toggle = (key: string, value: boolean) => {
    const next = { ...open, [key]: value };
    setOpen(next);
    try {
      localStorage.setItem(openKey(analysisId), JSON.stringify(next));
    } catch {
      /* Storage unavailable: the levels stay as toggled on this page. */
    }
  };
  // A file of the pull request that the Files view renders opens there; any other file on GitHub at the analysed
  // commit (generated, too large or binary files show no lines in the Files view).
  const links: DepsLinks = {
    line: (t) =>
      files.files.some(
        (f) => (t.side === "base" ? (f.old_path ?? f.path) : f.path) === t.file && f.not_rendered === null,
      )
        ? { href: `${base}/files${filesHash(t)}`, external: false }
        : {
            href: githubLineUrl(
              pr,
              { path: t.file, side: t.side, start: t.line, end: t.end ?? t.line },
              false,
            ),
            external: true,
          },
    flow: (id) => `${base}#${encodeURIComponent(id)}`,
    settings: GITHUB_SYNC_SETTINGS_PATH,
    onNavigate: (href) => void navigate(href),
  };
  let body: ReactNode;
  if (deps.isError && deps.error instanceof ApiError && deps.error.code === "analysis_not_finished") {
    return <Navigate to={`${base}/progress`} replace />;
  } else if (deps.isError) {
    body = (
      <>
        <StatusMessage kind="error">
          The dependencies could not be read: {errorText(deps.error)}
        </StatusMessage>
        <Tooltip tip="Reads the dependencies again.">
          <button
            type="button"
            aria-disabled={deps.isFetching || undefined}
            className={`${buttonClass("plain")} w-fit`}
            onClick={() => !deps.isFetching && void deps.refetch()}
          >
            Retry
          </button>
        </Tooltip>
      </>
    );
  } else if (!deps.data) {
    body = <StatusMessage kind="loading">Loading the dependencies…</StatusMessage>;
  } else {
    body = (
      <>
        <DependencyNotices view={deps.data} prOwner={pr.repo.owner} links={links} />
        {deps.data.status === "available" && (
          <DependencyTree view={deps.data} open={open} onToggle={toggle} links={links} />
        )}
      </>
    );
  }
  return (
    <div className="flex min-h-screen flex-col">
      <a
        href="#main"
        onClick={(event) => {
          event.preventDefault();
          mainRef.current?.focus();
        }}
        className="absolute -left-[999px] z-[100] bg-surface px-2.5 py-1.5 focus:top-2 focus:left-2"
      >
        Skip to content
      </a>
      <ReviewTopBar
        pr={pr}
        ci={tour.pr_context.facts.ci}
        checksHref={checksHref(pr)}
        language={tour.analysis.language}
        login={login}
        view={{
          current: "deps",
          flowsHref: base,
          filesHref: `${base}/files`,
          depsHref: `${base}/deps`,
          commentsHref: commentsPath(pr),
          prContextHref: `${base}#pr_context`,
          onNavigate: (href) => void navigate(href),
        }}
      />
      <main
        ref={mainRef}
        id="main"
        tabIndex={-1}
        className="grid max-w-[1400px] content-start gap-3 px-8 py-5"
      >
        <h1 className="text-[22px] font-bold">Dependencies</h1>
        <p className="m-0 text-muted">
          Crates this pull request changes, what they depend on in your organisations, and what was added for
          other crates&apos; types.
        </p>
        {body}
      </main>
    </div>
  );
}
