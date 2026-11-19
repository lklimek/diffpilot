import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Navigate, useLocation, useNavigate, useParams } from "react-router";
import { ApiError } from "../../api/client";
import type {
  AnalysisId,
  ApiErrorCode,
  AskAnchor,
  AskQuestion,
  CommentTarget,
  Evidence,
  FilesResponse,
  FileHunk,
  LineSide,
  LinesResponse,
  PrFile,
  StepId,
  Tour,
} from "../../api/generated/types";
import {
  useCommentActions,
  useFiles,
  useFlowWork,
  useMe,
  usePendingReview,
  useQuestionEvents,
  useSetFileViewed,
  useTour,
} from "../../api/queries";
import { useReviewStateStore } from "../../api/review-state";
import type { AskSubject } from "../../components/ask/AskForm";
import type { AskFilter } from "../../components/ask/AskThread";
import { BlockNav } from "../../components/block-nav/BlockNav";
import type { MarkerTarget } from "../../components/code-view/markers";
import { FileHeader, type ViewedControl } from "../../components/files/FileHeader";
import { FileTree } from "../../components/files/FileTree";
import {
  displaySide,
  flowsAt,
  hunkAt,
  hunkLabel,
  hunkLink,
  nextFile,
  parseFilesHash,
  pathOn,
  selectionStep,
  stepFilesLink,
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
import { rangeText } from "../../components/section/format";
import { StatusMessage } from "../../components/status/StatusMessage";
import { TopBar } from "../../components/top-bar/TopBar";
import { DiffCode, errorText, useHideContextChanges, type CodeContext } from "../tour/code";
import { useCommentDrafts, useUnresolvedPosts } from "../tour/drafts";
import { InspectorPane, type CommentLineState, type FilesInspector } from "../tour/InspectorPane";
import { loadLayout, saveLayout, showColumn, type TourLayout } from "../tour/layout";
import { Workspace } from "../tour/Workspace";
import { FileCode } from "./FileCode";

const checksHref = (pr: { html_url: string }) => `${pr.html_url.replace(/\/+$/, "")}/checks`;
const fileName = (path: string) => path.slice(path.lastIndexOf("/") + 1);
const short = (sha: string) => sha.slice(0, 7);
const lastKey = (id: AnalysisId) => `diffpilot.files.last.${id}`;
const transient = (error: unknown) =>
  !(error instanceof ApiError) || error.status === 0 || error.status >= 500;

// Shortcuts are ignored while the user types or holds a modifier.
function isTyping(event: KeyboardEvent): boolean {
  const target = event.target as HTMLElement | null;
  return (
    event.altKey ||
    event.ctrlKey ||
    event.metaKey ||
    target?.isContentEditable === true ||
    ["INPUT", "TEXTAREA", "SELECT"].includes(target?.tagName ?? "")
  );
}

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
        <a href="/">My work</a>
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
      login={login}
    />
  );
}

function Shell({ login, children }: { login: string | null; children: ReactNode }) {
  return (
    <div className="flex min-h-screen flex-col">
      <TopBar login={login} />
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

function FilesView({ analysisId, tour, files, ticked, login }: FilesViewProps) {
  const location = useLocation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [hideContextChanges, changeContextVisibility] = useHideContextChanges(login);
  const stream = useQuestionEvents(analysisId);
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
  const commentOn = (line: CommentLineState) => {
    setCommentLine(line);
    setCommentWithSelection(false);
  };
  const [focusNext, setFocusNext] = useState<"ask" | "comment" | { heading: string } | null>(null);
  const [notice, setNotice] = useState<{ text: string; nonce: number } | null>(null);
  const [viewedError, setViewedError] = useState<{ path: string; message: string } | null>(null);
  const [inspectorFlash, setInspectorFlash] = useState<(MarkerTarget & { nonce: number }) | null>(null);
  const flashNonce = useRef(0);
  const openPath = useRef<string | null>(null);
  const anchor = useRef<{ side: LineSide; line: number } | null>(null);
  const mainRef = useRef<HTMLDivElement>(null);
  const askRef = useRef<HTMLTextAreaElement>(null);
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
  const [missing] = useState(() =>
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
    if (target.path !== file.path) {
      setOldCode(false);
      setCommentLine(null);
      setViewedError(null);
      mainRef.current?.scrollTo?.({ top: 0 });
    }
    go({ file: target.path, side: displaySide(target) });
  };
  const select = (next: FilesSubject, of: PrFile = file) => go(subjectLink(of, next));

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
  // The review screen never scrolls the page; only the columns do.
  useEffect(() => {
    const style = document.documentElement.style;
    const before = style.scrollbarGutter;
    style.scrollbarGutter = "auto";
    return () => {
      style.scrollbarGutter = before;
    };
  }, []);
  useEffect(() => {
    const field =
      focusNext === "ask"
        ? askRef.current
        : focusNext === "comment"
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
  const path = pathOn(file, side);
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
      .find((d) => d && d.path === pathOn(file, side) && d.side === side && d.from <= line && line <= d.to);

  const selectLines = (target: CommentTarget, how: { extend: boolean }, data: LinesResponse) => {
    const from = how.extend && anchor.current?.side === target.side ? anchor.current.line : target.line;
    if (!how.extend || anchor.current?.side !== target.side)
      anchor.current = { side: target.side, line: target.line };
    const [a, b] = [Math.min(from, target.line), Math.max(from, target.line)];
    go({ file: file.path, side: target.side, line: a, end: b });
    // The composer targets the last line without taking the focus (Q16).
    setCommentLine({ target: { ...target, start_line: null, line: b }, lines: data });
    setCommentWithSelection(true);
  };
  const openAsk = () => {
    openInspector();
    sections.reveal("ask");
    setFocusNext("ask");
  };
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
    flashNonce.current += 1;
    setInspectorFlash({ ...target, nonce: flashNonce.current });
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
          setViewedError({ path: at.path, message: errorText(error) });
          // The PR moved on, or the mark may have changed: read the list again (head, marks).
          const code: ApiErrorCode | null = error instanceof ApiError ? error.code : null;
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
    error: viewedError?.path === file.path ? { message: viewedError.message, retry: toggleViewed } : null,
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

  const ctx: CodeContext = {
    hideContextChanges,
    analysisId,
    threads: tour.threads,
    comments: pending.data?.comments ?? [],
    activeLine: commentLine?.target ?? null,
    onComment: (target, data) => {
      commentOn({ target, lines: data });
      openInspector();
    },
    onLineSelect: selectLines,
    // "?" on a line selects it and opens Ask about it (A5).
    onAskLine: (target) => {
      go({ file: file.path, side: target.side, line: target.start_line ?? target.line, end: target.line });
      openAsk();
    },
    revealComment: null,
    onEditComment: comments.edit,
    onDeleteComment: comments.remove,
    onShowInInspector: showInInspector,
  };

  // Ask: the subject's lines, with their step when every changed line lies in one step (A4).
  const stepLabel = (id: StepId) => {
    const at = stepPlace(tour, id);
    return at ? ` · flow ${at.flow.position} · step ${at.step.number}` : "";
  };
  const linesSubject = (at: { side: LineSide; from: number; to: number }, noun: string): AskSubject => {
    const owner = selectionStep(file, at.side, at.from, at.to);
    const step = owner ? stepPlace(tour, owner)?.step : undefined;
    // R14 reads a step anchor's lines on the step's side: removed lines of a head step stay a lines anchor.
    const inside =
      step &&
      step.facts.side === at.side &&
      step.facts.range.start_line <= at.from &&
      at.to <= step.facts.range.end_line;
    const range = { start_line: at.from, end_line: at.to };
    const anchor: AskAnchor =
      step && inside
        ? { kind: "step", step_id: step.id, lines: range }
        : { kind: "lines", path: pathOn(file, at.side), side: at.side, range };
    return {
      anchor,
      label: `${pathOn(file, at.side)}:${rangeText(range)}${at.side === "base" ? " (removed)" : ""}${inside && owner ? stepLabel(owner) : ""}`,
      noun,
    };
  };
  const ask: AskSubject = narrowed
    ? {
        anchor: { kind: "step", step_id: narrowed.step.id, lines: null },
        label: `Step ${narrowed.step.number} · ${narrowed.step.llm.title}`,
        noun: "this step",
      }
    : lines
      ? linesSubject(lines, subject.kind === "hunk" ? "this hunk" : "these lines")
      : {
          anchor: { kind: "lines", path: file.path, side, range: null },
          label: file.path,
          noun: "this file",
        };

  const onFile = (q: AskQuestion) =>
    q.anchor.kind === "lines"
      ? q.anchor.path === file.path || q.anchor.path === file.old_path
      : stepPlace(tour, q.anchor.step_id)?.step.facts.file === path;
  const blockSteps = new Set(
    (hunk
      ? flowsAt(tour, hunk.parts)
      : lines
        ? flowsAt(
            tour,
            file.hunks
              .flatMap((h) => h.parts)
              .filter(
                (p) =>
                  p.side === lines.side && p.range.start_line <= lines.to && p.range.end_line >= lines.from,
              ),
          )
        : []
    ).flatMap((f) => f.steps.map((s) => s.step.id)),
  );
  const inBlock = (q: AskQuestion) => {
    if (narrowed) return q.anchor.kind === "step" && q.anchor.step_id === narrowed.step.id;
    if (!lines) return false;
    if (q.anchor.kind === "step") return blockSteps.has(q.anchor.step_id);
    const r = q.anchor.range;
    return (
      onFile(q) &&
      q.anchor.side === lines.side &&
      (r === null || (r.start_line <= lines.to && r.end_line >= lines.from))
    );
  };
  const askFilters: AskFilter[] = [
    {
      key: "file",
      label: "This file",
      tip: "Shows only the questions about this file: its lines, or a step on it.",
      empty: "No question about this file yet. Whole conversation shows the others.",
      match: onFile,
    },
    ...(subject.kind !== "file"
      ? [
          {
            key: "block",
            label: "This block",
            tip: "Shows only the questions about the selected hunk, step or lines.",
            empty: "No question about this block yet. Whole conversation shows the others.",
            match: inBlock,
          },
        ]
      : []),
  ];
  const placeOfQuestion = (q: AskQuestion) => {
    if (q.anchor.kind === "lines") {
      const anchorFile = files.files.find(
        (f) =>
          f.path === (q.anchor as { path: string }).path ||
          f.old_path === (q.anchor as { path: string }).path,
      );
      if (!anchorFile) return null;
      const a = q.anchor;
      return {
        label: `${a.path}${a.range ? `:${rangeText(a.range)}` : ""} · in Files`,
        open: () =>
          go({
            file: anchorFile.path,
            side: a.side,
            ...(a.range ? { line: a.range.start_line, end: a.range.end_line } : {}),
          }),
      };
    }
    const at = stepPlace(tour, q.anchor.step_id);
    if (!at) return null;
    const lines = q.anchor.lines;
    return {
      label: `${at.step.facts.file}${lines ? `:${rangeText(lines)}` : ""} · flow ${at.flow.position} · step ${at.step.number}`,
      open: () => {
        const target = stepFilesLink(at.step);
        if (fileOf(files.files, target)) go(target);
        else openInFlows(at.step.id);
      },
    };
  };

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
    ask,
    askFilters,
    commentHint:
      "Select lines by their numbers in the code (Shift+click or drag extends); the comment goes on the last selected line (c). Any displayed line can take a comment, with or without an LLM remark.",
    isCurrent: subject.kind === "file" ? onFile : inBlock,
    feedback,
    diff,
  };

  const next = hunkStep(1);
  const previous = hunkStep(-1);
  const hunkTitle = (x: { file: PrFile; hunk: FileHunk }) => `${fileName(x.file.path)} ${hunkLabel(x.hunk)}`;
  const centre = (
    <>
      {missing && (
        <p role="status" className="mb-2 rounded-md border border-border bg-surface px-3 py-1 text-[12.5px]">
          {missing}
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
            askLineHint="Selects the line and opens Ask in the Inspector; the answer is LLM output."
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
          />
        )}
      </div>
      <BlockNav
        previous={previous && hunkTitle(previous)}
        next={next && hunkTitle(next)}
        onPrevious={() => goHunk(previous)}
        onNext={() => goHunk(next)}
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
      <TopBar
        pr={tour.analysis.pr}
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
        pendingCount={
          pending.isSuccess
            ? pending.data.commenting_available
              ? pending.data.comments.length
              : "unavailable"
            : pending.isPending
              ? "loading"
              : undefined
        }
        onOpenPending={() => void navigate(`${flowsHref}#pending_review`)}
        onHelp={() => setHelpOpen(true)}
        view={{
          current: "files",
          flowsHref: flowsTarget(),
          filesHref: `${location.pathname}${location.hash}`,
          prContextHref: `${flowsHref}#pr_context`,
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
            streamLost={stream.lost}
            analysisId={analysisId}
            tour={tour}
            block={narrowed ? { block: "step", step_id: narrowed.step.id } : { block: "pr_context" }}
            step={narrowed?.step}
            annotationId={null}
            flash={inspectorFlash}
            onFlashed={() => setInspectorFlash(null)}
            onBackToStep={back}
            onSelectStep={openInFlows}
            placeOf={() => null}
            placeOfQuestion={placeOfQuestion}
            onEvidence={onEvidence}
            codeShown
            pending={pending}
            comments={comments}
            commentLine={commentLine}
            onCancelComment={() => setCommentLine(null)}
            unresolved={unresolved}
            onUnresolvedChange={setUnresolved}
            drafts={drafts}
            reanalyseHref={`/review?pr=${encodeURIComponent(tour.analysis.pr.html_url)}`}
            askRef={askRef}
            sections={sections}
            commentRef={commentRef}
            askHeight={layout.askHeight}
            onAskHeight={(askHeight) => setLayout({ ...layout, askHeight })}
            askInsert={null}
            onAskInserted={() => {}}
            commentPrefill={null}
            onCommentPrefilled={() => {}}
            files={filesInspector}
          />
        }
      />
      <HelpDialog open={helpOpen} onClose={() => setHelpOpen(false)} shortcuts={FILES_SHORTCUTS} />
    </div>
  );
}
