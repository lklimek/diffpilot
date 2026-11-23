import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import { log, resetReviewLoop } from "../../mocks/review-loop";
import { DecisionLog } from "./DecisionLog";
import { EmptyQueueCard } from "./QueueCards";
import { agentPrompt, StartAgentHelp } from "./StartAgentHelp";

const NOW = new Date("2026-11-18T09:27:00Z");
const PR = { repo: { owner: "acme", name: "node-client" }, number: 482 };
const ORIGIN = "http://agentic:3000";
const PROMPT =
  "Use the diffpilot-decisions skill to carry out my decisions on acme/node-client#482 from diffpilot at " +
  "http://agentic:3000: subscribe, claim each decision, carry out its effects in this repository's checkout of " +
  "the PR branch, and report each effect done or failed with the commit or comment link.";

/** `document.execCommand("copy")` over plain HTTP: fires a copy event when `works`; returns what it wrote. */
function insecureCopy(works: boolean): Map<string, string> {
  vi.stubGlobal("isSecureContext", false);
  const data = new Map<string, string>();
  const execCommand = (command: string) => {
    if (!works || command !== "copy") return false;
    const event = new Event("copy", { cancelable: true }) as ClipboardEvent;
    Object.defineProperty(event, "clipboardData", {
      value: { setData: (type: string, value: string) => data.set(type, value) },
    });
    document.dispatchEvent(event);
    return true;
  };
  Object.defineProperty(document, "execCommand", { value: execCommand, configurable: true });
  return data;
}

beforeEach(() => {
  resetReviewLoop();
  window.localStorage.clear();
});

afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "execCommand");
});

describe("StartAgentHelp (owner 2026-11-23)", () => {
  it("the prompt names this pull request and this diffpilot, never a token", () => {
    expect(agentPrompt(PR, ORIGIN)).toBe(PROMPT);
  });

  it("says what an agent needs, shows the prompt and copies it as plain text only", async () => {
    const data = insecureCopy(true);
    render(<StartAgentHelp pr={PR} origin={ORIGIN} collapsed={false} />);
    const help = screen.getByRole("region", { name: "How to start an agent" });
    expect(help).toHaveTextContent("Decided effects wait until an agent claims their decision.");
    expect(help).toHaveTextContent(
      "an access token with the review_loop_execute scope (its checkbox in Settings › Access tokens)",
    );
    expect(help).toHaveTextContent(
      "the skill reads it from ~/envs/diffpilot-agent.env (SR_AGENT_KEY, SR_URL; plain http needs SR_HTTP_HOST), never from the chat",
    );
    expect(help).toHaveTextContent("docs/contracts/agent-api.md §7");
    expect(within(help).getByText(PROMPT)).toBeVisible();
    const copy = within(help).getByRole("button", { name: "Copy prompt" });
    expect(copy).toHaveAccessibleDescription(/Claude Code session/);
    await userEvent.click(copy);
    expect(Object.fromEntries(data)).toEqual({ "text/plain": PROMPT });
    expect(within(help).getByRole("status")).toHaveTextContent("Copied the prompt.");
    expectTrustWording(document.body.textContent ?? "");
  });

  it("a copy that did not work says to select the prompt", async () => {
    insecureCopy(false);
    render(<StartAgentHelp pr={PR} origin={ORIGIN} collapsed={false} />);
    await userEvent.click(screen.getByRole("button", { name: "Copy prompt" }));
    expect(screen.getByRole("status")).toHaveTextContent(
      "Copying did not work in this browser. Select the prompt and copy it yourself.",
    );
  });

  it("collapsed: a closed disclosure that opens on the same text", async () => {
    render(<StartAgentHelp pr={PR} origin={ORIGIN} collapsed />);
    const details = screen.getByText("How to start an agent").closest("details")!;
    expect(details).not.toHaveAttribute("open");
    await userEvent.click(screen.getByText("How to start an agent"));
    expect(details).toHaveAttribute("open");
    expect(within(details).getByText(PROMPT)).toBeVisible();
  });
});

describe("How to start an agent on the Decision log and the empty queue", () => {
  it("the Decision feed shows it while no agent is subscribed, else collapsed", () => {
    const { unmount } = render(
      <DecisionLog log={{ ...log(), subscribers: [] }} pr={PR} now={NOW} storageKey="t" origin={ORIGIN} />,
    );
    const feed = () => screen.getByRole("region", { name: "Decision feed" });
    expect(within(feed()).getByRole("region", { name: "How to start an agent" })).toBeVisible();
    unmount();
    render(<DecisionLog log={log()} pr={PR} now={NOW} storageKey="t" origin={ORIGIN} />);
    expect(screen.queryByRole("region", { name: "How to start an agent" })).toBeNull();
    expect(within(feed()).getByText("How to start an agent").closest("details")).not.toHaveAttribute("open");
  });

  it("the empty queue carries what the screen arranges in it", () => {
    render(
      <EmptyQueueCard
        read={{
          read_at: "2026-11-18T09:20:00Z",
          threads: 14,
          complete: true,
          reviews: 4,
          comments: 3,
          failure: null,
          next_read_at: null,
        }}
        assessed={14}
        head="52d2da1"
        now={NOW}
      >
        <StartAgentHelp pr={PR} origin={ORIGIN} collapsed={false} />
      </EmptyQueueCard>,
    );
    const empty = screen.getByRole("region", { name: "No undecided questions" });
    expect(within(empty).getByRole("region", { name: "How to start an agent" })).toBeVisible();
  });
});
