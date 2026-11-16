import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useParams } from "react-router";
import { ApiError } from "../../api/client";
import type { BlockRef, RangeQuery, Tour } from "../../api/generated/types";
import { useLines, useMe, useReviewState, useTour } from "../../api/queries";
import { BlockNav } from "../../components/block-nav/BlockNav";
import { CodeView } from "../../components/code-view/CodeView";
import { FlowIntroBlock } from "../../components/flow-intro/FlowIntroBlock";
import { NavList } from "../../components/nav/NavList";
import { PrContextBlock } from "../../components/pr-context/PrContextBlock";
import { StatusMessage } from "../../components/status/StatusMessage";
import { StepBlock } from "../../components/step/StepBlock";
import { TopBar } from "../../components/top-bar/TopBar";
import { blockTitle, buildSequence, findStep, sameBlock } from "./sequence";

const PR_CONTEXT: BlockRef = { block: "pr_context" };

function errorText(error: unknown): string {
  if (error instanceof ApiError && error.code === "analysis_not_finished") {
    return "This analysis has not finished yet; the review opens when it has.";
  }
  return error instanceof ApiError ? error.message : "Something went wrong. Reload the page.";
}

// Ignore j / k while the user types or uses modifier keys.
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

function StepCode({ analysisId, tour, stepId }: { analysisId: string; tour: Tour; stepId: string }) {
  const found = findStep(tour, stepId);
  const query: RangeQuery | undefined = found && {
    path: found.step.facts.file,
    side: found.step.facts.side,
    from: found.step.facts.range.start_line,
    to: found.step.facts.range.end_line,
  };
  const lines = useLines(analysisId, query);
  if (lines.isPending) return <StatusMessage kind="loading">Loading the code…</StatusMessage>;
  if (lines.isError)
    return <StatusMessage kind="error">Cannot show the code: {errorText(lines.error)}</StatusMessage>;
  return <CodeView lines={lines.data} annotations={found?.step.llm.annotations ?? []} />;
}

/** The review screen (thin slice): left menu and one block at a time with Previous / Next and `j` / `k`. */
export function TourScreen() {
  const { analysisId = "" } = useParams();
  const me = useMe();
  const tour = useTour(analysisId);
  const reviewState = useReviewState(analysisId);
  const [chosen, setChosen] = useState<BlockRef | null>(null);
  const main = useRef<HTMLElement>(null);

  const sequence = useMemo(() => (tour.data ? buildSequence(tour.data) : []), [tour.data]);
  const lastViewed = reviewState.data?.last_viewed;
  const resumed = lastViewed && sequence.some((b) => sameBlock(b, lastViewed)) ? lastViewed : PR_CONTEXT;
  const current = chosen ?? resumed;
  const index = sequence.findIndex((b) => sameBlock(b, current));
  const previous = index > 0 ? sequence[index - 1] : undefined;
  const next = index >= 0 ? sequence[index + 1] : undefined;

  const go = (block: BlockRef | undefined) => {
    if (!block) return;
    setChosen(block);
    main.current?.scrollTo({ top: 0 });
  };

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || isTyping(event)) return;
      if (event.key === "j") go(next);
      else if (event.key === "k") go(previous);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  });

  const login = me.data?.user?.login ?? null;
  if (tour.isPending || (reviewState.isPending && !chosen)) {
    return (
      <Shell login={login}>
        <StatusMessage kind="loading">Loading the review…</StatusMessage>
      </Shell>
    );
  }
  if (tour.isError) {
    return (
      <Shell login={login}>
        <StatusMessage kind="error">{errorText(tour.error)}</StatusMessage>
        <a href="/">Back to Home</a>
      </Shell>
    );
  }

  const data = tour.data;
  let block;
  if (current.block === "flow_intro") {
    const flow = data.flows.find((f) => f.id === current.flow_id);
    block = flow && (
      <FlowIntroBlock
        flow={flow}
        dependencies={data.flows.filter((f) => flow.llm.depends_on.includes(f.id))}
      />
    );
  } else if (current.block === "step") {
    const found = findStep(data, current.step_id);
    block = found && (
      <StepBlock
        key={found.step.id}
        step={found.step}
        code={<StepCode analysisId={analysisId} tour={data} stepId={found.step.id} />}
      />
    );
  } else {
    block = <PrContextBlock pr={data.analysis.pr} context={data.pr_context} />;
  }

  return (
    <div className="flex h-screen flex-col">
      <a
        href="#main"
        className="absolute -left-[999px] z-[100] bg-surface px-2.5 py-1.5 focus:top-2 focus:left-2"
      >
        Skip to content
      </a>
      <TopBar pr={data.analysis.pr} ci={data.pr_context.facts.ci} login={login} />
      <div className="grid min-h-0 flex-1 grid-cols-1 md:grid-cols-[var(--width-nav)_minmax(0,1fr)]">
        <nav aria-label="Flows and steps" className="overflow-auto border-r border-border bg-surface p-3.5">
          <NavList flows={data.flows} current={current} onSelect={go} />
        </nav>
        <main id="main" ref={main} tabIndex={-1} className="overflow-auto px-6 pt-4 pb-10">
          {block ?? <StatusMessage kind="empty">This block is not part of the analysis.</StatusMessage>}
          <BlockNav
            previous={previous && blockTitle(data, previous)}
            next={next && blockTitle(data, next)}
            onPrevious={() => go(previous)}
            onNext={() => go(next)}
          />
          <p className="sr-only" aria-live="polite">
            {blockTitle(data, current)}
          </p>
        </main>
      </div>
    </div>
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
