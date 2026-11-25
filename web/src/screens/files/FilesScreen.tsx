import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Navigate, useLocation, useNavigate, useParams } from "react-router";
import { ApiError } from "../../api/client";
import { routes } from "../../api/generated/routes";
import type {
  AnalysisId,
  ApiErrorCode,
  AskAnchor,
  Annotation,
  CommentTarget,
  Evidence,
  FilesResponse,
  FileHunk,
  LineRange,
  LineSide,
  LinesResponse,
  PrFile,
  StepId,
  Tour,
} from "../../api/generated/types";
import {
  useCommentActions,
  useFiles,
  linesQuery,
  useFlowWork,
  useMe,
  usePendingReview,
  useSetFileViewed,
  useTour,
} from "../../api/queries";
import { useReviewStateStore } from "../../api/review-state";
import { BlockNav } from "../../components/block-nav/BlockNav";
import type { MarkerTarget } from "../../components/code-view/markers";
import { FileHeader, type ViewedControl } from "../../components/files/FileHeader";
import { FileTree } from "../../components/files/FileTree";
import {
  displaySide,
  fileName,
  flowsAt,
  hunkAt,
  hunkLabel,
  hunkLink,
  nextFile,
  parseFilesHash,
  pathOn,
  selectionStep,
  stepPlace,
  subjectLink,
  subjectOf,
  treeOrder,
  filesHash,
  type FilesLink,
  type FilesSubject,
} from "../../components/files/model";
import {
  filePanel,
  hunkPanel,
  linesPanel,
  stepInHunkPanel,
  type FilesPanelContext,
} from "../../components/files/panels";
import { isFlowReviewed } from "../../components/flow-intro/progress";
import { HelpDialog } from "../../components/help/HelpDialog";
import { FILES_SHORTCUTS, shortcutAction } from "../../components/help/shortcuts";
import { useSectionState } from "../../components/inspector/sectionState";
import { StatusMessage } from "../../components/status/StatusMessage";
import { ReviewTopBar } from "../shell/ReviewTopBar";
import { DiffCode, errorText, MAX_RANGE_LINES, useHideContextChanges, type CodeContext } from "../tour/code";
import { flagComment } from "../../components/comment/prefill";
import { useCommentDrafts, useUnresolvedPosts } from "../tour/drafts";
import { InspectorPane, type CommentLineState, type FilesInspector } from "../tour/InspectorPane";
import { loadLayout, saveLayout, showColumn, type TourLayout } from "../tour/layout";
import {
  checksHref,
  isTyping,
  openGithubLine,
  pendingCountOf,
  transient,
  useNoPageScroll,
} from "../tour/review";
import { FlowsMenu } from "../tour/FlowsMenu";
import { Workspace } from "../tour/Workspace";
import { useAskChat, useAskPlace, type AnswerDraft } from "../review/askChat";
import { rangeText } from "../../components/section/format";
import { FileCode } from "./FileCode";
import { commentsPath, prPath } from "../pr/paths";

const short = (sha: string) => sha.slice(0, 7);
const lastKey = (id: AnalysisId) => `diffpilot.files.last.${id}`;

/** `/analyses/{id}/files` (brief §4.5): file tree, one file, Inspector; the same analysis as the Flows view. */
export function FilesScreen() {
  const { analysisId = "" } = useParams();
  const me = useMe();
  const tour = useTour(analysisId);
  const files = useFiles(analysisId);
  const review = useReviewStateStore(analysisId);
  const login = me.data?.user?.login ?? null;
  const notFinished = [tour.error, files.error].some(
    (e) => e instanceof ApiError && e.code === "analysis_not_finished",
  );
  if (notFinished) return <Navigate to={`/analyses/${encodeURIComponent(analysisId)}/progress`} replace />;
  const failed = [tour, files].find((q) => q.isError && (!q.data || !transient(q.error)));
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
  if (!tour.data || !files.data || review.status === "loading") {
    return (
      <Shell login={login}>
        <StatusMessage kind="loading">Loading the files…</StatusMessage>
      </Shell>
    );
  }
  if (files.data.files.length === 0) {
    return (
      <Shell login={login}>
        <StatusMessage kind="empty">This pull request has no files to show.</StatusMessage>
      </Shell>
    );
  }
  return (
    <FilesView
      key={analysisId}
      analysisId={analysisId}
      tour={tour.data}
      files={files.data}
      ticked={review.state.ticked_steps}
      ticksUnread={review.status === "unreadable" ? errorText(review.loadError) : null}
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

interface FilesViewProps {
  analysisId: AnalysisId;
  tour: Tour;
  files: FilesResponse;
  ticked: readonly StepId[];
  /** Why the step ticks could not be read; null when they were. */
  ticksUnread: string | null;
  login: string | null;
}

/** The file of `link` (by its head path, or its base path for a link to removed lines). */
const fileOf = (files: readonly PrFile[], link: FilesLink | undefined) =>
  link && files.find((f) => f.path === link.file || (link.side === "base" && f.old_path === link.file));

/** The subject's lines on one side, for the classic diff, Ask and the code view. */
function subjectLines(
  file: PrFile,
  subject: FilesSubject,
): { side: LineSide; from: number; to: number } | null {
  if (subject.kind === "lines") return { side: subject.side, from: subject.from, to: subject.to };
  if (subject.kind !== "hunk") return null;
  const hunk = file.hunks.find((h) => h.id === subject.id);
  if (!hunk) return null;
  const l = hunkLink(hunk);
  return { side: l.side, from: l.start_line, to: l.end_line };
}

function FilesView({ analysisId, tour, files, ticked, ticksUnread, login }: FilesViewProps) {
  const location = useLocation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [hideContextChanges, changeContextVisibility] = useHideContextChanges(login);
  const chat = useAskChat();
  const pending = usePendingReview(analysisId);
  const comments = useCommentActions(analysisId);
  const work = useFlowWork(analysisId);
  const setViewed = useSetFileViewed(analysisId);
  const [unresolved, setUnresolved] = useUnresolvedPosts(analysisId, login);
  const drafts = useCommentDrafts(analysisId, login);
  const [layout, setLayoutState] = useState(loadLayout);
  const [filter, setFilter] = useState("");
  const [oldCode, setOldCode] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [commentLine, setCommentLine] = useState<CommentLineState | null>(null);
  // The composer's line came with a line selection (not `c` or a button): Esc closes both at once.
  const [commentWithSelection, setCommentWithSelection] = useState(false);
  const [commentPrefill, setCommentPrefill] = useState<{ text: string } | null>(null);
  const commentOn = (line: CommentLineState) => {
    setCommentLine(line);
    setCommentWithSelection(false);
  };
  const [focusNext, setFocusNext] = useState<"comment" | { heading: string } | null>(null);
  const [notice, setNotice] = useState<{ text: string; nonce: number } | null>(null);
  const [viewedError, setViewedError] = useState<{
    path: string;
    message: string;
    signedOut: boolean;
  } | null>(null);
  const [inspectorFlash, setInspectorFlash] = useState<(MarkerTarget & { nonce: number }) | null>(null);
  const openPath = useRef<string | null>(null);
  // Where Shift+click and drags extend from: the last line number pressed in this file.
  const anchor = useRef<{ path: string; side: LineSide; line: number } | null>(null);
  // Lines of the open file by side, once a range of it was read (for links past its end).
  const [totals, setTotals] = useState<Record<string, number>>({});
  const mainRef = useRef<HTMLDivElement>(null);
  const commentRef = useRef<HTMLTextAreaElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const say = (text: string) => setNotice((n) => ({ text, nonce: (n?.nonce ?? 0) + 1 }));

  const ordered = useMemo(() => treeOrder(files.files), [files.files]);
  // The URL decides what is open; without a fragment, the last place of this analysis in this browser (Q3).
  const stored = useMemo(() => {
    try {
      return parseFilesHash(localStorage.getItem(lastKey(analysisId)) ?? "");
    } catch {
      return undefined;
    }
  }, [analysisId]);
  const asked = parseFilesHash(location.hash) ?? (location.hash ? undefined : stored);
  const known = fileOf(files.files, asked);
  // FilesScreen renders this view only for a pull request with files.
  const file = known ?? ordered[0]!;
  const link: FilesLink = known
    ? { ...asked!, file: known.path }
    : { file: file.path, side: displaySide(file) };
  const subject = subjectOf(file, link);
  const hash = filesHash(link);
  // A factual note about the link that opened the view; gone once the reader goes elsewhere.
  const [linkNotice, setLinkNotice] = useState(() =>
    asked && !known ? `This link names ${asked.file}, which is not a file of this pull request.` : null,
  );

  const setLayout = (next: TourLayout) => {
    saveLayout(next);
    setLayoutState(next);
  };
  const openInspector = () => {
    if (layout.full !== "inspector" && (layout.full !== null || layout.hidden.inspector))
      setLayout(showColumn(layout, "inspector"));
  };
  const sections = useSectionState(JSON.stringify([file.path, subject]));

  // One history entry for the view: every selection replaces the location (proposal §6).
  const go = (next: FilesLink) => {
    if (filesHash(next) !== location.hash)
      void navigate({ ...location, hash: filesHash(next) }, { replace: true });
  };
  const open = (path: string) => {
    const target = files.files.find((f) => f.path === path);
    if (!target) return;
    setLinkNotice(null);
    if (target.path !== file.path) {
      anchor.current = null;
      setOldCode(false);
      setCommentLine(null);
      setViewedError(null);
      mainRef.current?.scrollTo?.({ top: 0 });
    }
    go({ file: target.path, side: displaySide(target) });
  };
  const select = (next: FilesSubject, of: PrFile = file) => {
    setLinkNotice(null);
    go(subjectLink(of, next));
  };

  // The location always names what is shown (a missing or unknown fragment becomes the file shown).
  useEffect(() => {
    if (location.hash !== hash && (!parseFilesHash(location.hash) || !known))
      void navigate({ ...location, hash }, { replace: true });
  }, [hash, known, location, navigate]);
  useEffect(() => {
    try {
      localStorage.setItem(lastKey(analysisId), hash);
    } catch {
      /* Storage unavailable: the URL still holds the place. */
    }
  }, [analysisId, hash]);
  useEffect(() => {
    openPath.current = file.path;
  });
  const locationNotice = (location.state as { notice?: string } | null)?.notice ?? null;
  // A link past the end of the file: say so and open the file; a range running past it ends at the last line.
  const total = totals[`${file.path}:${displaySide(file)}`];
  useEffect(() => {
    if (total === undefined || subject.kind !== "lines" || subject.side !== displaySide(file)) return;
    if (subject.from > total) {
      // The notice travels with the location; the next selection replaces it.
      const notice = `Line ${subject.from} is outside ${file.path} at this analysis (${total} ${total === 1 ? "line" : "lines"}).`;
      void navigate(
        { ...location, hash: filesHash({ file: file.path, side: subject.side }) },
        { replace: true, state: { notice } },
      );
    } else if (subject.to > total) {
      const link = { file: file.path, side: subject.side, line: subject.from, end: total };
      void navigate({ ...location, hash: filesHash(link) }, { replace: true });
    }
  }, [total, subject, file, location, navigate]);
  useNoPageScroll();
  useEffect(() => {
    const field =
      focusNext === "comment"
        ? commentRef.current
        : focusNext && typeof focusNext === "object" && focusNext.heading === file.path
          ? headingRef.current
          : null;
    if (!field) return;
    field.focus();
    setFocusNext(null);
  }, [focusNext, layout, file.path]);

  const flowsHref = `/analyses/${analysisId}`;
  // Where `s` and the view menu land in the Flows view: the narrowed step, else the first step at the subject.
  const flowsTarget = (): string => {
    if (subject.kind === "hunk" && subject.step) return `${flowsHref}#${encodeURIComponent(subject.step)}`;
    const at = subjectLines(file, subject);
    const parts = at
      ? file.hunks
          .flatMap((h) => h.parts)
          .filter((p) => p.side === at.side && p.range.start_line <= at.to && p.range.end_line >= at.from)
      : file.hunks.flatMap((h) => h.parts);
    const first = flowsAt(tour, parts)[0]?.steps[0];
    if (first) return `${flowsHref}#${encodeURIComponent(first.step.id)}`;
    if (file.group.kind === "not_analysed") return `${flowsHref}#${file.group.category}`;
    if (parts.some((p) => p.owner.kind === "unexplained")) return `${flowsHref}#unexplained`;
    if (parts.some((p) => p.owner.kind === "identical_move")) return `${flowsHref}#moves`;
    return flowsHref;
  };
  const openInFlows = (step: StepId) => void navigate(`${flowsHref}#${encodeURIComponent(step)}`);

  const side = displaySide(file);
  const hunk = subject.kind === "hunk" ? file.hunks.find((h) => h.id === subject.id) : undefined;
  const narrowed = subject.kind === "hunk" && subject.step ? stepPlace(tour, subject.step) : undefined;
  // A narrowed step's own lines in the hunk (head, else removed): its diff, its marks in the code.
  const stepHere =
    narrowed && hunk
      ? flowsAt(tour, hunk.parts)
          .flatMap((f) => f.steps)
          .find((s) => s.step.id === narrowed.step.id)
      : undefined;
  const stepRanges = stepHere && (stepHere.lines.length > 0 ? stepHere.lines : stepHere.removed);
  const lines =
    stepHere && stepRanges && stepRanges.length > 0
      ? {
          side: (stepHere.lines.length > 0 ? "head" : "base") as LineSide,
          from: stepRanges[0]!.start_line,
          to: stepRanges.at(-1)!.end_line,
        }
      : subjectLines(file, subject);

  // Remarks of every analysed step on this file's side: one ◆ each, named with their step.
  // ... and of steps on its removed code (base numbers), drawn on the removed rows of the head view.
  const stepsOn = (on: LineSide) =>
    tour.flows.flatMap((f) =>
      f.steps
        .filter((s) => s.facts.side === on && s.facts.file === pathOn(file, on))
        .map((s) => ({ flow: f, step: s })),
    );
  const steps = stepsOn(side);
  const removedSteps = side === "head" ? stepsOn("base") : [];
  const ownerOf = new Map(
    [...steps, ...removedSteps].flatMap(({ flow, step }) =>
      step.llm.annotations.map((a) => [a.id, { flow, step }] as const),
    ),
  );
  const annotations = steps.flatMap(({ step }) => step.llm.annotations);
  const removedAnnotations = removedSteps.flatMap(({ step }) => step.llm.annotations);

  // A changed line's displayed range in the cache (for `c` without a clicked line).
  const cachedLines = (side: LineSide, line: number): LinesResponse | undefined =>
    queryClient
      .getQueriesData<LinesResponse>({ queryKey: ["lines", analysisId] })
      .map(([, data]) => data)
      .find(
        (d) =>
          d &&
          ((d.path === pathOn(file, side) && d.side === side && d.from <= line && line <= d.to) ||
            // A removed line is displayed inside the head range that holds its block.
            (side === "base" &&
              d.side === "head" &&
              d.path === pathOn(file, "head") &&
              d.deleted.some((b) => b.base_start <= line && line < b.base_start + b.lines.length))),
      );

  const selectLines = (target: CommentTarget, how: { extend: boolean }, data: LinesResponse) => {
    setLinkNotice(null);
    // Without a pressed line in this file, a linked selection on the same side is the anchor.
    const own = anchor.current?.path === file.path && anchor.current.side === target.side;
    const start = own
      ? anchor.current!.line
      : subject.kind === "lines" && subject.side === target.side
        ? subject.from
        : null;
    const from = how.extend && start !== null ? start : target.line;
    if (!how.extend || start === null)
      anchor.current = { path: file.path, side: target.side, line: target.line };
    else if (!own) anchor.current = { path: file.path, side: target.side, line: start };
    const [a, b] = [Math.min(from, target.line), Math.max(from, target.line)];
    go({ file: file.path, side: target.side, line: a, end: b });
    // The composer targets the last line without taking the focus (Q16).
    setCommentLine({ target: { ...target, start_line: null, line: b }, lines: data });
    setCommentWithSelection(true);
  };
  const openAsk = () => chat.open();
  const hunkOfStep = (id: StepId) =>
    file.hunks.find((h) =>
      h.parts.some((p) => (p.owner.kind === "step" && p.owner.step_id === id) || p.also_in.includes(id)),
    );
  const details = (id: StepId) => {
    const at = hunk ?? hunkOfStep(id);
    if (at) select({ kind: "hunk", id: at.id, step: id });
  };
  const showInInspector = (target: MarkerTarget) => {
    if (target.kind === "annotation") {
      const owner = ownerOf.get(target.id);
      const annotation = owner?.step.llm.annotations.find((a) => a.id === target.id);
      const at = annotation && owner && hunkAt(file, owner.step.facts.side, annotation.range.start_line);
      const into = at ?? (owner ? hunkOfStep(owner.step.id) : undefined);
      if (owner && into) select({ kind: "hunk", id: into.id, step: owner.step.id });
    } else {
      const thread = tour.threads.find((t) => t.id === target.id);
      const at = thread?.line != null && thread.side ? hunkAt(file, thread.side, thread.line) : undefined;
      if (at) select({ kind: "hunk", id: at.id, ...(thread?.step_id ? { step: thread.step_id } : {}) });
    }
    setInspectorFlash((flash) => ({ ...target, nonce: (flash?.nonce ?? 0) + 1 }));
    openInspector();
  };

  // Hunks across files in tree order, for j / k and r.
  const allHunks = ordered.flatMap((f) => (f.not_rendered ? [] : f.hunks.map((h) => ({ file: f, hunk: h }))));
  // The open hunk (`exact`), or the first hunk at or after the selection, the file's start or the next file.
  const position = (() => {
    if (hunk) {
      const at = allHunks.findIndex((x) => x.hunk.id === hunk.id && x.file.path === file.path);
      return { at, exact: true };
    }
    const start = lines?.from ?? 0;
    let at = allHunks.findIndex((x) => x.file.path === file.path && hunkLink(x.hunk).end_line >= start);
    if (at < 0) at = allHunks.findIndex((x) => ordered.indexOf(x.file) > ordered.indexOf(file));
    return { at: at < 0 ? allHunks.length : at, exact: false };
  })();
  const hunkStep = (delta: 1 | -1) =>
    allHunks[delta > 0 ? position.at + (position.exact ? 1 : 0) : position.at - 1];
  const goHunk = (x: { file: PrFile; hunk: FileHunk } | undefined) => {
    if (!x) return;
    if (x.file.path !== file.path) {
      setOldCode(false);
      setCommentLine(null);
    }
    select({ kind: "hunk", id: x.hunk.id }, x.file);
  };
  const remarked = (x: { hunk: FileHunk }) =>
    flowsAt(tour, x.hunk.parts).some((f) =>
      f.steps.some((s) => s.role === "explained" && s.step.llm.annotations.length > 0),
    );

  // Viewed: the mark changes only after GitHub confirmed; a mark then opens the next file (Q6, A1).
  const headMoved = files.current_head !== null && files.current_head !== tour.analysis.pr.head_sha;
  const toggleViewed = () => {
    const at = file;
    const mark = at.viewed !== "viewed";
    setViewedError(null);
    setViewed.mutate(
      { path: at.path, viewed: mark },
      {
        onSuccess: () => {
          if (!mark) return say(`Removed the Viewed mark of ${fileName(at.path)} on GitHub.`);
          const next = nextFile(files.files, at.path);
          // The next file opens only while the reader is still on the file they marked.
          if (!next || openPath.current !== at.path)
            return say(`Marked ${fileName(at.path)} as viewed on GitHub.`);
          open(next.path);
          setFocusNext({ heading: next.path });
          say(
            `Marked ${fileName(at.path)} as viewed on GitHub. Opened the next file, ${fileName(next.path)}.`,
          );
        },
        onError: (error) => {
          const code: ApiErrorCode | null = error instanceof ApiError ? error.code : null;
          setViewedError({ path: at.path, message: errorText(error), signedOut: code === "unauthenticated" });
          // The PR moved on, or the mark may have changed: read the list again (head, marks).
          if (code === "pr_head_changed" || code === "github_unavailable")
            void queryClient.invalidateQueries({ queryKey: ["files", analysisId] });
        },
      },
    );
  };
  const viewed: ViewedControl = {
    state: file.viewed,
    saving: setViewed.isPending && setViewed.variables?.path === file.path,
    disabledReason: headMoved
      ? `This analysis is for ${short(tour.analysis.pr.head_sha)}; the PR is now at ${short(files.current_head!)}. Marking Viewed would apply to newer code on GitHub.`
      : file.viewed === null
        ? files.viewed_status === "partial"
          ? "Viewed state not read: GitHub lists only the first 3000 files of a pull request. Mark it on GitHub."
          : "Viewed state not read from GitHub (GitHub did not answer); nothing is assumed. Mark it on GitHub."
        : setViewed.isPending && setViewed.variables?.path !== file.path
          ? `Saving the Viewed mark of ${fileName(setViewed.variables!.path)} to GitHub; this file can be marked once GitHub confirmed it.`
          : null,
    error:
      viewedError?.path === file.path
        ? {
            message: viewedError.message,
            retry: toggleViewed,
            signIn: viewedError.signedOut ? routes.authLogin.path : undefined,
          }
        : null,
    onToggle: toggleViewed,
  };

  // `c`: the open comment, else the selection's last line or the hunk's first line.
  const commentOnCurrentLine = () => {
    if (commentLine) {
      openInspector();
      return setFocusNext("comment");
    }
    if (!lines) return;
    const line = subject.kind === "lines" ? lines.to : lines.from;
    const data = cachedLines(lines.side, line);
    if (!data) return;
    commentOn({
      target: { path: pathOn(file, lines.side), side: lines.side, start_line: null, line },
      lines: data,
    });
    openInspector();
    setFocusNext("comment");
  };
  const back = () => {
    if (subject.kind === "hunk" && subject.step) return select({ kind: "hunk", id: subject.id });
    if (subject.kind !== "file") select({ kind: "file" });
  };

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || helpOpen || isTyping(event)) return;
      switch (shortcutAction(event.key, FILES_SHORTCUTS)) {
        case "next_block":
          return goHunk(hunkStep(1));
        case "previous_block":
          return goHunk(hunkStep(-1));
        case "next_flow": {
          const next = nextFile(files.files, file.path);
          return next && open(next.path);
        }
        case "previous_flow": {
          const previous = nextFile(files.files, file.path, -1);
          return previous && open(previous.path);
        }
        case "toggle_viewed":
          if (!viewed.saving && !viewed.disabledReason) toggleViewed();
          return;
        case "toggle_old_code":
          if (!file.not_rendered) setOldCode((s) => !s);
          return;
        case "github_line":
          event.preventDefault();
          return openGithubLine(
            mainRef.current,
            lines
              ? {
                  side: lines.side,
                  line: subject.kind === "lines" ? lines.to : lines.from,
                }
              : undefined,
          );
        case "comment":
          event.preventDefault();
          return commentOnCurrentLine();
        case "next_remark": {
          const from = position.at + (position.exact ? 1 : 0);
          const next = [...allHunks.slice(from), ...allHunks.slice(0, from)].find(remarked);
          if (next) goHunk(next);
          else say("No more hunks with LLM remarks");
          return;
        }
        case "ask":
          event.preventDefault();
          return openAsk();
        case "switch_view":
          return void navigate(flowsTarget());
        case "help":
          return setHelpOpen(true);
        case "close":
          if (commentLine && !commentWithSelection) setCommentLine(null);
          else if (subject.kind !== "file") {
            if (commentWithSelection) setCommentLine(null);
            back();
          } else if (commentLine) setCommentLine(null);
          else if (layout.full) setLayout({ ...layout, full: null });
          return;
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  });

  const askRemark = (remark: Annotation) => {
    const facts = ownerOf.get(remark.id)?.step.facts;
    if (facts)
      chat.open({
        anchor: { kind: "lines", path: facts.file, side: facts.side, range: remark.range },
        remark,
      });
  };
  const ctx: CodeContext = {
    hideContextChanges,
    analysisId,
    pr: tour.analysis.pr,
    files: files.files,
    threads: tour.threads,
    comments: pending.data?.comments ?? [],
    activeLine: commentLine?.target ?? null,
    onComment: (target, data) => {
      commentOn({ target, lines: data });
      openInspector();
    },
    onLineSelect: selectLines,
    // "?" on a line selects it and opens the Ask chat about it (A5); the chat's subject follows the selection.
    onAskLine: (target) => {
      go({ file: file.path, side: target.side, line: target.start_line ?? target.line, end: target.line });
      openAsk();
    },
    revealComment: null,
    onEditComment: comments.edit,
    onDeleteComment: comments.remove,
    onShowInInspector: showInInspector,
    onAskRemark: askRemark,
  };

  // Ask: the subject's lines, with their step when every changed line lies in one step (A4).
  const linesAnchor = (at: { side: LineSide; from: number; to: number }): AskAnchor => {
    const owner = selectionStep(file, at.side, at.from, at.to);
    const step = owner ? stepPlace(tour, owner)?.step : undefined;
    // R14 reads a step anchor's lines on the step's side: removed lines of a head step stay a lines anchor.
    const inside =
      step &&
      step.facts.side === at.side &&
      step.facts.range.start_line <= at.from &&
      at.to <= step.facts.range.end_line;
    const range = { start_line: at.from, end_line: at.to };
    return step && inside
      ? { kind: "step", step_id: step.id, lines: range }
      : { kind: "lines", path: pathOn(file, at.side), side: at.side, range };
  };
  // The Ask chat's subject follows the selection: a narrowed step, the hunk's or selected lines, else the file.
  const askPlace: AskAnchor = narrowed
    ? { kind: "step", step_id: narrowed.step.id, lines: null }
    : lines
      ? linesAnchor(lines)
      : { kind: "lines", path: file.path, side, range: null };
  useAskPlace(askPlace);

  // Feedback rates one step's LLM analysis.
  const explained = hunk
    ? flowsAt(tour, hunk.parts)
        .flatMap((f) => f.steps)
        .filter((s) => s.role === "explained")
    : [];
  const lineStep =
    subject.kind === "lines" ? selectionStep(file, subject.side, subject.from, subject.to) : null;
  const rated =
    narrowed?.step ??
    (explained.length === 1 ? explained[0]!.step : lineStep ? stepPlace(tour, lineStep)?.step : undefined);
  const feedback: FilesInspector["feedback"] = rated
    ? { target: { target: "step", step_id: rated.id }, subject: `step ${rated.number}` }
    : {
        none:
          explained.length > 1
            ? "Feedback rates one step: pick Details on a step."
            : "Feedback rates the LLM analysis of one step; select a hunk or step that has some.",
      };

  const diff = lines && (
    <DiffCode
      analysisId={analysisId}
      query={{ path: pathOn(file, lines.side), side: lines.side, from: lines.from, to: lines.to }}
      comments={{
        comments: pending.data?.comments ?? [],
        pr: tour.analysis.pr,
        files: files.files,
        onEditComment: comments.edit,
        onDeleteComment: comments.remove,
      }}
    />
  );
  const workAllowed = ["planning", "running", "paused", "finished"].includes(tour.analysis.state.state);
  const onEvidence = (e: Evidence) => {
    const target = fileOf(files.files, { file: e.file, side: e.side });
    if (target) go({ file: target.path, side: e.side, line: e.line });
  };
  // The composer on the lines with `text` as a draft, focused; nothing is sent.
  const draftAt =
    (path: string, side: LineSide, start_line: number, end_line: number, text: string) =>
    (data: LinesResponse) => {
      commentOn({
        target: { path, side, start_line: start_line === end_line ? null : start_line, line: end_line },
        lines: data,
      });
      setCommentPrefill({ text });
      openInspector();
      setFocusNext("comment");
    };
  // Lines folded away (or of another file) are read first.
  const readLines = (path: string, side: LineSide, start_line: number, end_line: number) =>
    queryClient.fetchQuery(
      linesQuery(analysisId, {
        path,
        side,
        from: Math.max(start_line, end_line - MAX_RANGE_LINES + 1),
        to: end_line,
      }),
    );
  const draftOn = (
    path: string,
    side: LineSide,
    start_line: number,
    end_line: number,
    text: string,
    what: string,
    done?: () => void,
  ) => {
    const placed = draftAt(path, side, start_line, end_line, text);
    const draft = (data: LinesResponse) => {
      placed(data);
      done?.();
    };
    const cached = path === pathOn(file, side) ? cachedLines(side, end_line) : undefined;
    if (cached) return draft(cached);
    readLines(path, side, start_line, end_line).then(draft, (error: unknown) =>
      setLinkNotice(`${what} could not be read: ${errorText(error)}`),
    );
  };
  const onFlagAsComment =
    pending.data?.commenting_available === true
      ? (flag: Annotation) => {
          const owner = ownerOf.get(flag.id)?.step;
          if (!owner) return;
          const side = owner.facts.side;
          const { start_line, end_line } = flag.range;
          draftOn(pathOn(file, side), side, start_line, end_line, flagComment(flag), "The flag's lines");
        }
      : undefined;
  // "Use as review comment" on an answer: the draft goes on the lines it was asked about and the composer takes the
  // focus (AC12); lines of another file open that file first, and the move is said.
  const movedText = (path: string, range: LineRange) =>
    `Opened ${path}:${rangeText(range)} to comment on its lines; the draft is in the Comment composer.`;
  const answerAsComment = (anchor: AskAnchor, text: string) => {
    const step = anchor.kind === "step" ? stepPlace(tour, anchor.step_id)?.step : undefined;
    const at =
      anchor.kind === "lines" && anchor.range
        ? { path: anchor.path, side: anchor.side, range: anchor.range }
        : step && {
            path: step.facts.file,
            side: step.facts.side,
            range: anchor.kind === "step" && anchor.lines ? anchor.lines : step.facts.range,
          };
    if (!at) return;
    const { start_line, end_line } = at.range;
    const other =
      at.path === pathOn(file, at.side) ? undefined : fileOf(files.files, { file: at.path, side: at.side });
    if (other) go({ file: other.path, side: at.side, line: start_line, end: end_line });
    else if (at.path !== pathOn(file, at.side)) return;
    draftOn(at.path, at.side, start_line, end_line, text, "The answer's lines", () => {
      if (other) chat.announce(movedText(at.path, at.range));
    });
  };
  // A draft handed over from Flows ("Use as review comment" on an answer about lines of a file) is taken once.
  const incoming = (location.state as { commentDraft?: AnswerDraft } | null)?.commentDraft;
  const taken = useRef<string | null>(null);
  useEffect(() => {
    chat.setActions({ onEvidence, onUseAsComment: answerAsComment });
    if (!incoming || taken.current === location.key) return;
    taken.current = location.key;
    void navigate({ ...location }, { replace: true, state: null });
    const { path, side, range, text } = incoming;
    readLines(path, side, range.start_line, range.end_line).then(
      (data) => {
        draftAt(path, side, range.start_line, range.end_line, text)(data);
        chat.announce(movedText(path, range));
      },
      (error: unknown) => setLinkNotice(`The answer's lines could not be read: ${errorText(error)}`),
    );
  });
  const panelContext: FilesPanelContext = {
    tour,
    file,
    pr: tour.analysis.pr,
    ticked,
    sections,
    onSelectHunk: (id) => select({ kind: "hunk", id }),
    onDetails: details,
    onOpenInFlows: openInFlows,
    onRequestFlow: workAllowed ? (flow) => work.request.mutate(flow) : undefined,
    requestBusy: work.request.isPending,
    diff,
    onEvidence,
    onFlagAsComment,
    onAskRemark: askRemark,
    onFlag: (flag) => showInInspector({ kind: "annotation", id: flag.id }),
  };
  const panel =
    narrowed && hunk
      ? stepInHunkPanel(panelContext, hunk, narrowed.step.id)
      : hunk
        ? hunkPanel(panelContext, hunk)
        : subject.kind === "lines"
          ? linesPanel(panelContext, subject, {
              onAsk: openAsk,
              onComment: () => {
                const data = cachedLines(subject.side, subject.to);
                if (data)
                  commentOn({
                    target: {
                      path: pathOn(file, subject.side),
                      side: subject.side,
                      start_line: null,
                      line: subject.to,
                    },
                    lines: data,
                  });
                openInspector();
                setFocusNext("comment");
              },
            })
          : filePanel(panelContext);
  const filesInspector: FilesInspector = {
    panel,
    back:
      subject.kind === "file"
        ? undefined
        : narrowed
          ? {
              label: "← Back to hunk",
              tip: "Shows the whole hunk again: every flow and step in it (Esc).",
              context: hunk ? `${fileName(file.path)} · hunk, ${hunkLabel(hunk)}` : undefined,
              onBack: back,
            }
          : {
              label: "← Back to file",
              tip: "Shows the file summary again (Esc).",
              context: file.path,
              onBack: back,
            },
    commentHint:
      "Select lines by their numbers in the code (Shift+click or drag extends); the comment goes on the last selected line (c). Any displayed line can take a comment, with or without an LLM remark.",
    feedback,
    diff,
  };

  const next = hunkStep(1);
  const previous = hunkStep(-1);
  const hunkTitle = (x: { file: PrFile; hunk: FileHunk }) => `${fileName(x.file.path)} ${hunkLabel(x.hunk)}`;
  const centre = (
    <>
      {(linkNotice ?? locationNotice) && (
        <p role="status" className="mb-2 rounded-md border border-border bg-surface px-3 py-1 text-[12.5px]">
          {linkNotice ?? locationNotice}
        </p>
      )}
      {ticksUnread && (
        <p
          role="alert"
          className="mb-2 rounded-md border border-l-4 border-border border-l-med-bd bg-surface px-3 py-1 text-[12.5px]"
        >
          Your step ticks could not be read, so ticks shown here may be missing: {ticksUnread}
        </p>
      )}
      <FileHeader
        key={file.path}
        file={file}
        pr={tour.analysis.pr}
        viewed={viewed}
        headingRef={headingRef}
        oldCode={file.not_rendered ? undefined : { shown: oldCode, onToggle: () => setOldCode((s) => !s) }}
      />
      {!file.not_rendered && (
        <p className="mt-1.5 mb-0 text-[12px] text-muted">
          Click a hunk for its flows. Click, Shift+click or drag line numbers to select lines as your own
          block; the link is in the address bar.
        </p>
      )}
      <div className="mt-2">
        {file.not_rendered ? (
          <StatusMessage kind="empty">
            Not shown:{" "}
            {file.not_rendered === "binary"
              ? "a binary file"
              : file.not_rendered === "generated"
                ? "a generated file"
                : "larger than the code reader's limit"}
            . The PR context counts its changes (+{file.added} −{file.removed}).
          </StatusMessage>
        ) : (
          <FileCode
            key={file.path}
            file={file}
            ctx={ctx}
            annotations={annotations}
            removedAnnotations={removedAnnotations}
            askLineHint="Selects the line and opens the Ask chat about it; the answer is LLM output."
            bandOf={(a) => ownerOf.get(a.id)?.step.llm.assessment?.risk ?? null}
            remarkOwner={(a) => {
              const owner = ownerOf.get(a.id);
              return owner ? `flow ${owner.flow.position} · step ${owner.step.number}` : null;
            }}
            hunkOf={(s, line) => hunkAt(file, s, line)?.id ?? null}
            selectedHunk={hunk?.id ?? null}
            onSelectHunk={(id) => select({ kind: "hunk", id })}
            selection={
              (subject.kind === "lines" || stepHere) && lines
                ? { side: lines.side, from: lines.from, to: lines.to }
                : null
            }
            show={
              subject.kind === "lines" && subject.side === side
                ? { start_line: subject.from, end_line: subject.to }
                : undefined
            }
            revealLine={lines && lines.side === side ? lines.from : undefined}
            oldCode={oldCode}
            onTotalLines={(n) =>
              setTotals((all) =>
                all[`${file.path}:${side}`] === n ? all : { ...all, [`${file.path}:${side}`]: n },
              )
            }
          />
        )}
      </div>
      <BlockNav
        previous={previous && hunkTitle(previous)}
        next={next && hunkTitle(next)}
        onPrevious={() => goHunk(previous)}
        onNext={() => goHunk(next)}
        keys
      />
      {notice && (
        <p key={notice.nonce} role="status" className="sr-only">
          {notice.text}
        </p>
      )}
    </>
  );

  return (
    <div className="flex h-dvh flex-col overflow-hidden">
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
        pr={tour.analysis.pr}
        currentPath={file.path}
        ci={tour.pr_context.facts.ci}
        checksHref={checksHref(tour.analysis.pr)}
        language={tour.analysis.language}
        login={login}
        hideContextChanges={hideContextChanges}
        onHideContextChanges={changeContextVisibility}
        progress={{
          reviewed: tour.flows.filter((f) => isFlowReviewed(f, [...ticked])).length,
          total: tour.flows.length,
        }}
        progressMenu={
          <FlowsMenu
            tour={tour}
            ticked={ticked}
            work={work}
            onOpenFlow={(flowId) => void navigate(`${flowsHref}#${encodeURIComponent(flowId)}`)}
          />
        }
        pendingCount={pendingCountOf(pending)}
        onOpenPending={() => void navigate(`${flowsHref}#pending_review`)}
        onHelp={() => setHelpOpen(true)}
        view={{
          current: "files",
          flowsHref: flowsTarget(),
          filesHref: `${location.pathname}${location.hash}`,
          depsHref: `/analyses/${analysisId}/deps`,
          commentsHref: commentsPath(tour.analysis.pr),
          prPageHref: prPath(tour.analysis.pr),
          onNavigate: (href) => void navigate(href),
        }}
      />
      <Workspace
        layout={layout}
        onChange={setLayout}
        mainRef={mainRef}
        titles={{ nav: "Files", main: "File" }}
        navLabel="Files"
        nav={
          <FileTree
            files={files.files}
            tour={tour}
            ticked={ticked}
            current={file.path}
            viewedStatus={files.viewed_status}
            filter={filter}
            onFilter={setFilter}
            onSelect={open}
          />
        }
        main={centre}
        inspector={
          <InspectorPane
            analysisId={analysisId}
            tour={tour}
            block={narrowed ? { block: "step", step_id: narrowed.step.id } : { block: "pr_context" }}
            step={narrowed?.step}
            annotationId={null}
            flash={inspectorFlash}
            onFlashed={() => setInspectorFlash(null)}
            onBackToStep={back}
            onSelectStep={openInFlows}
            onEvidence={onEvidence}
            codeShown
            pending={pending}
            comments={comments}
            commentLine={commentLine}
            onCancelComment={() => setCommentLine(null)}
            unresolved={unresolved}
            onUnresolvedChange={setUnresolved}
            drafts={drafts}
            reanalyseHref={prPath(tour.analysis.pr)}
            sections={sections}
            commentRef={commentRef}
            commentPrefill={commentPrefill}
            onCommentPrefilled={() => setCommentPrefill(null)}
            files={filesInspector}
          />
        }
      />
      <HelpDialog open={helpOpen} onClose={() => setHelpOpen(false)} shortcuts={FILES_SHORTCUTS} />
    </div>
  );
}
