import { useId } from "react";
import type { PrRef } from "../../api/generated/types";
import { CopyTextButton } from "../dashboard/common/CopyTextButton";

const TITLE = "How to start an agent";

/**
 * The prompt for a Claude Code session that carries out the decisions on `pr` from the diffpilot at `origin` (owner
 * 2026-11-23). It holds no token: the skill reads it from the agent's env file. It names no capabilities: the skill
 * subscribes to every effect kind, so no decision is left unclaimable. An agent claims a decision, then reports
 * each of its effects.
 */
export const agentPrompt = (pr: PrRef, origin: string): string =>
  `Use the diffpilot-decisions skill to carry out my decisions on ${pr.repo.owner}/${pr.repo.name}#${pr.number} ` +
  `from diffpilot at ${origin}: subscribe, claim each decision, carry out its effects in this repository's ` +
  `checkout of the PR branch, and report each effect done or failed with the commit or comment link.`;

function Steps({ pr, origin }: { pr: PrRef; origin: string }) {
  const prompt = agentPrompt(pr, origin);
  return (
    <>
      <p className="m-0">
        Decided effects wait until an agent claims their decision. An agent needs an access token with the{" "}
        <b>review_loop_execute</b> scope (its checkbox in Settings › Account › Access tokens). In a checkout
        of this repository, start a Claude Code session with this prompt:
      </p>
      <pre
        data-agent-prompt
        className="my-1.5 rounded border border-border bg-surface-2 px-2 py-1.5 font-mono text-xs whitespace-pre-wrap"
      >
        {prompt}
      </pre>
      <CopyTextButton
        text={prompt}
        label="Copy prompt"
        what="the prompt"
        tip="Copies the prompt to paste into a Claude Code session; it holds no token."
      />
      <p className="mt-1.5 mb-0 text-xs text-muted">
        The prompt holds no token: the skill reads it from{" "}
        <code className="font-mono">~/envs/diffpilot.env</code> (
        <code className="font-mono">SR_AGENT_KEY</code>, <code className="font-mono">SR_URL</code>; plain http
        needs <code className="font-mono">SR_HTTP_HOST</code>), never from the chat. The agent API is
        described in <code className="font-mono">docs/contracts/agent-api.md</code> §7 of diffpilot.
      </p>
    </>
  );
}

/**
 * How to start an agent that carries out the decisions on `pr`: why effects wait, the token it needs and a prompt
 * to copy. A section of its own, or a closed disclosure when `collapsed` (an agent is already subscribed).
 */
export function StartAgentHelp({ pr, origin, collapsed }: { pr: PrRef; origin: string; collapsed: boolean }) {
  const id = useId();
  if (collapsed)
    return (
      <details className="mt-2 text-[13px]">
        <summary className="min-h-6 cursor-pointer font-semibold">{TITLE}</summary>
        <div className="mt-1">
          <Steps pr={pr} origin={origin} />
        </div>
      </details>
    );
  return (
    <section
      aria-labelledby={`${id}-h`}
      className="mt-2 rounded-md border border-border bg-surface px-3 py-2 text-[13px]"
    >
      <h3 id={`${id}-h`} className="mt-0 mb-1 text-sm font-semibold">
        {TITLE}
      </h3>
      <Steps pr={pr} origin={origin} />
    </section>
  );
}
