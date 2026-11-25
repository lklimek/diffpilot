// What the review-loop pages show and send, checked against the server's contract (crates/server/src/review_loop/
// effects.rs `plan`, queue.rs `decide_rest`); the msw mocks model that contract (mocks/effects-plan.ts).
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import type { CommentItem, DecideRequest, Gate } from "../../api/generated/types";
import { COMMENTS_POLL_MS } from "../../api/review-loop";
import { DISMISS_MESSAGE, dismissBody } from "../../mocks/effects-plan";
import { BOT_CHANGE_REQUESTS, listItems, resetReviewLoop } from "../../mocks/review-loop";
import { setScenario } from "../../mocks/scenarios";

const server = startMockServer();
const PR = "/pr/acme/node-client/482";
const byNo = (no: number) => listItems().find((i) => i.no === no)!;

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
  resetReviewLoop();
  window.localStorage.clear();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

const PENDING = "Planning the effects of your current input; you can decide once they are shown.";

/** RC16 answers wait while `hold` is set (then the mocks' server plans as usual). */
function holdPreviews() {
  let release: (() => void) | null = null;
  let held: Promise<void> | null = null;
  server.use(
    http.post("/api/v1/prs/:owner/:name/:number/comments/effects-preview", async () => {
      if (held) await held;
    }),
  );
  return {
    hold: () => (held = new Promise((done) => (release = done))),
    release: () => {
      held = null;
      release?.();
    },
  };
}

/** Every item thepastaclaw raised gets a standing decision, and the dismissal question listing its review is asked. */
function askStaleDismissal(): CommentItem {
  const standing = listItems().flatMap((i) => i.decisions)[0]!;
  for (const i of listItems().filter((i) => i.author === "thepastaclaw" && i.state !== "outside"))
    Object.assign(i, { state: "decided", decisions: [standing] });
  const question: CommentItem = {
    ...byNo(24),
    id: "00000000-0000-4000-8000-000000000030",
    no: 30,
    stable_key: "pr:3:dismiss",
    title: "Dismiss stale bot reviews?",
    body: dismissBody(BOT_CHANGE_REQUESTS),
  };
  listItems().push(question);
  return question;
}

/** effects.rs `EffectItem::on_review`: a PR-level comment answering a review starts with a line naming it. */
function onReview(item: CommentItem, text: string): string {
  if (item.kind !== "review_body" || !item.url) return text;
  const line =
    item.author_kind === "human"
      ? `@${item.author}, re [your review](${item.url}):`
      : `Re [${item.author}'s review](${item.url}):`;
  return `${line}\n${text}`;
}

/** The server's default reply texts (effects.rs `plan`); an edited `reply_text` replaces them. */
function postedReply(item: CommentItem, request: DecideRequest): string | null {
  const edited = request.reply_text?.trim() || null;
  const reply = (text: string) => edited ?? onReview(item, text);
  const reason = request.reason?.trim() ?? "";
  const gate: Gate | null = request.override?.type === "blocker_gate" ? request.override.gate : null;
  switch (request.kind) {
    case "resolve":
      return item.author_kind === "human"
        ? reply("Per maintainer decision, this is already addressed in this pull request.")
        : null;
    case "fix_here":
      return reply("Per maintainer decision, this is fixed in this pull request.");
    case "reject":
      return reply(
        gate
          ? `Per maintainer decision, accepted despite ${gate}: ${reason}`
          : `Per maintainer decision, not changed: ${reason}`,
      );
    default:
      throw new Error(`not covered here: ${request.kind}`);
  }
}

/** The decision requests the page sends (RC8), read from the client's fetch calls. */
function sentDecisions(): DecideRequest[] {
  const sent: DecideRequest[] = [];
  const fetch = globalThis.fetch;
  vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
    const url = input instanceof Request ? input.url : String(input);
    if (url.endsWith("/decisions") && typeof init?.body === "string")
      sent.push(JSON.parse(init.body) as DecideRequest);
    return fetch(input, init);
  });
  return sent;
}

/** Enter outside a text field (the card's Decide key). */
const fireEnter = () =>
  act(() => {
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  });

/** Opens `C<no>` in Questions. */
async function open(no: number) {
  renderRoute(`${PR}/comments?item=C${no}`);
  await screen.findByRole("heading", { level: 2, name: new RegExp(`^C${no} · `) });
}

/** The reply preview once the server planned the current draft (`text` when given). */
async function settledPreview(text?: string) {
  await waitFor(() => {
    const group = screen.getByRole("group", { name: "Preview of the reply on GitHub" });
    expect(group).not.toHaveAttribute("aria-busy");
    if (text) expect(group).toHaveTextContent(text);
  });
  return preview();
}

const preview = () =>
  screen.queryByRole("group", { name: "Preview of the reply on GitHub" })?.querySelector("p")?.textContent ??
  null;

describe("Reply preview is what the server posts (plan §3.2 item 8; effects.rs plan)", () => {
  it.each([
    ["Fix here", 22, null, null],
    ["Fix here on a review body (the line naming the review)", 23, null, null],
    ["Reject", 11, null, null],
    ["Accept despite the gate", 14, /^Accept despite the gate/, "Pre-existing; tracked in #455."],
  ] as const)("%s: the previewed reply equals the posted one", async (_, no, option, reason) => {
    const item = byNo(no);
    const sent = sentDecisions();
    await open(no);
    if (option) await userEvent.click(screen.getByRole("radio", { name: option }));
    if (reason) await userEvent.type(screen.getByLabelText("Reason (posted as the public reply)"), reason);
    const shown = await settledPreview(reason ?? undefined);
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(shown).toBe(postedReply(item, sent[0]!));
    await waitFor(() => expect(byNo(no).decisions.at(-1)?.reply_text).toBe(shown));
  });

  it("Resolve on a human thread previews the reply the server posts", async () => {
    Object.assign(byNo(22), { author: "octo-reviewer", author_kind: "human" });
    const item = byNo(22);
    const sent = sentDecisions();
    await open(22);
    await userEvent.click(screen.getByRole("radio", { name: /^Resolve \(already fixed\)/ }));
    const shown = await settledPreview("already addressed");
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(shown).toBe(postedReply(item, sent[0]!));
  });

  it("Resolve on a bot thread: What happens next lists no reply (the server plans only the resolve)", async () => {
    await open(22);
    await userEvent.click(screen.getByRole("radio", { name: /^Resolve \(already fixed\)/ }));
    expect(await screen.findByText("resolves the thread.")).toBeVisible();
    const next = screen.getByRole("button", { name: "What happens next" }).closest("section, div")!;
    expect(within(next.parentElement!).queryByText(/replies/)).toBeNull();
  });
});

describe("Dismiss stale bot reviews (P3): the edited dismissal message reaches the server", () => {
  it("the typed message is planned and sent in the field the server posts (`reason`)", async () => {
    askStaleDismissal();
    const sent = sentDecisions();
    renderRoute(`${PR}/comments?item=P30`);
    await screen.findByRole("heading", { name: /Dismiss stale bot reviews\?/ });
    const field = screen.getByLabelText("Dismissal message (posted)");
    await waitFor(() => expect(field).toHaveValue(DISMISS_MESSAGE));
    await userEvent.clear(field);
    await userEvent.type(field, "Superseded by round 4.");
    const decide = screen.getByRole("button", { name: "Decide" });
    await waitFor(() => expect(decide).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(decide);
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]!.kind).toBe("dismiss_stale_reviews");
    // effects.rs: `let message = reason.unwrap_or(DISMISS_MESSAGE)`; `reply_text` is not read for P3.
    expect(sent[0]!.reason).toBe("Superseded by round 4.");
    await waitFor(() => expect(byNo(30).decisions).toHaveLength(1));
    expect(byNo(30).decisions[0]!.effects.map((e) => [e.kind, e.params.text])).toEqual([
      ["dismiss_review", "Superseded by round 4."],
    ]);
  });
});

describe("Merge with follow-ups (P1) leaves gate items to their own decision (mod.rs `Loaded::scope`)", () => {
  it("What happens next and the stored effects name no gate item; C14 and C15 stay in the queue", async () => {
    renderRoute(`${PR}/comments?item=P24`);
    await screen.findByRole("heading", { name: /Merge now, or another round\?/ });
    const created = await screen.findByText(/^creates one draft follow-up PR listing /);
    expect(created.textContent).toContain("C22");
    expect(created.textContent).not.toMatch(/C1[45]\b/);
    const next = created.closest("ol")!;
    expect(next.textContent).not.toMatch(/C1[45]\b/);
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    await waitFor(() => expect(byNo(24).decisions).toHaveLength(1));
    const touched = byNo(24).decisions[0]!.effects.flatMap((e) => e.params.item_nos);
    expect(touched).toContain(22);
    expect(touched).not.toContain(14);
    expect(touched).not.toContain(15);
    expect([byNo(14).state, byNo(15).state]).toEqual(["undecided", "undecided"]);
  });
});

describe("Dismiss stale bot reviews (P3) acts on the reviews its question lists (mod.rs `dismissal`)", () => {
  it("a listed review that is no longer stale: What happens next says why, in the server's words", async () => {
    listItems().push({
      ...byNo(24),
      id: "00000000-0000-4000-8000-000000000030",
      no: 30,
      stable_key: "pr:3:dismiss",
      title: "Dismiss stale bot reviews?",
      body: dismissBody(BOT_CHANGE_REQUESTS),
    });
    renderRoute(`${PR}/comments?item=P30`);
    await screen.findByRole("heading", { name: /Dismiss stale bot reviews\?/ });
    expect(
      await screen.findByText(
        "C30: the reviews this question lists changed since it was asked; keep the reviews, the next round asks again.",
      ),
    ).toHaveAttribute("role", "alert");
  });
});

describe("Decide sends only what the server planned for the current input (RC16)", () => {
  it("a target changed while its plan is pending is decided only once the server planned it", async () => {
    const previews = holdPreviews();
    const sent = sentDecisions();
    await open(22);
    await userEvent.click(screen.getByRole("radio", { name: /^Handled in another PR/ }));
    const target = screen.getByLabelText("Existing pull request");
    await userEvent.type(target, "490");
    await settledPreview("#490.");
    previews.hold();
    await userEvent.clear(target);
    await userEvent.type(target, "491{Enter}");
    const decide = screen.getByRole("button", { name: "Decide" });
    await userEvent.click(decide);
    fireEnter();
    expect(sent).toHaveLength(0);
    expect(decide).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText(PENDING)).toHaveAttribute("role", "status");
    previews.release();
    const shown = await settledPreview("#491.");
    await waitFor(() => expect(decide).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(decide);
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]!.target).toEqual({ type: "existing_pr", number: 491 });
    expect(shown).toBe("Per maintainer decision, this is addressed in #491.");
    await waitFor(() => expect(byNo(22).decisions.at(-1)?.reply_text).toBe(shown));
  });
});

describe("Merge now carries the digest of the confirmed plan (RC16 `plan_digest`, RC8 409 conflict)", () => {
  it("a comment that joined the scope unseen refuses the decision; the plan is made again and confirmed first", async () => {
    const sent = sentDecisions();
    renderRoute(`${PR}/comments?item=P24`);
    await screen.findByRole("heading", { name: /Merge now, or another round\?/ });
    const listing = () => screen.getByText(/^creates one draft follow-up PR listing /).textContent ?? "";
    await waitFor(() => expect(listing()).toContain("C22"));
    // Another reader's change, not yet read by this page.
    listItems().push({
      ...byNo(22),
      id: "00000000-0000-4000-8000-000000000030",
      no: 30,
      stable_key: "thread-30",
      decisions: [],
    });
    const decide = screen.getByRole("button", { name: "Decide" });
    await waitFor(() => expect(decide).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(decide);
    expect(
      await screen.findByText(
        "What the decision would do changed since you confirmed it: C24. Check the plan again.",
      ),
    ).toHaveAttribute("role", "alert");
    expect(sent[0]!.plan_digest).toEqual(expect.any(String));
    expect(byNo(24).decisions).toHaveLength(0);
    await waitFor(() => expect(listing()).toContain("C30"));
    expect(screen.getByText(/What this decision acts on changed since you looked/)).toBeVisible();
    await userEvent.click(decide);
    expect(sent).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: "Use the new plan" }));
    await userEvent.click(decide);
    await waitFor(() => expect(byNo(24).decisions).toHaveLength(1));
    expect(sent[1]!.plan_digest).not.toBe(sent[0]!.plan_digest);
    expect(byNo(24).decisions[0]!.effects.flatMap((e) => e.params.item_nos)).toContain(30);
  });
});

describe("A failed plan never leaves valid input blocked (RC16 unavailable)", () => {
  it("while RC16 answers 503 Decide waits with Plan again; once the server answers, Plan again makes it available", async () => {
    server.use(
      http.post("/api/v1/prs/:owner/:name/:number/comments/effects-preview", () =>
        HttpResponse.json({ code: "unavailable", message: "The service is unavailable." }, { status: 503 }),
      ),
    );
    const sent = sentDecisions();
    await open(22);
    const planAgain = await screen.findByRole("button", { name: "Plan again" });
    const decide = screen.getByRole("button", { name: "Decide" });
    expect(decide).toHaveAttribute("aria-disabled", "true");
    expect(decide).toHaveAccessibleDescription(expect.stringContaining("The service is unavailable"));
    await userEvent.click(decide);
    expect(sent).toHaveLength(0);
    server.resetHandlers();
    await userEvent.click(planAgain);
    await waitFor(() => expect(decide).not.toHaveAttribute("aria-disabled"));
    expect(screen.queryByRole("button", { name: "Plan again" })).toBeNull();
    await userEvent.click(decide);
    await waitFor(() => expect(sent).toHaveLength(1));
  });
});

describe("PR-level plans follow the PR: a P1 plan is made again when the items change (RC16)", () => {
  it("a comment found by the minute poll re-plans Merge with follow-ups; Decide waits until the owner confirms", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const sent = sentDecisions();
    renderRoute(`${PR}/comments?item=P24`);
    await screen.findByRole("heading", { name: /Merge now, or another round\?/ });
    const listing = () => screen.getByText(/^creates one draft follow-up PR listing /).textContent ?? "";
    await waitFor(() => expect(listing()).toContain("C22"));
    expect(listing()).not.toContain("C30");
    listItems().push({
      ...byNo(22),
      id: "00000000-0000-4000-8000-000000000030",
      no: 30,
      stable_key: "thread-30",
      decisions: [],
    });
    await act(() => vi.advanceTimersByTimeAsync(COMMENTS_POLL_MS));
    await waitFor(() => expect(listing()).toContain("C30"));
    expect(screen.getByText(/What this decision acts on changed since you looked/)).toHaveAttribute(
      "role",
      "status",
    );
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(sent).toHaveLength(0);
    await userEvent.click(screen.getByRole("button", { name: "Use the new plan" }));
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    await waitFor(() => expect(sent).toHaveLength(1));
    await waitFor(() => expect(byNo(24).decisions).toHaveLength(1));
    expect(byNo(24).decisions[0]!.effects.flatMap((e) => e.params.item_nos)).toContain(30);
  });
});

describe("Decide the rest per recommendation (plan §2.4; queue.rs decide_rest)", () => {
  it("a repeat asked again (C21) is left out like the server's list, not decided in bulk", async () => {
    renderRoute(`${PR}/comments`);
    await userEvent.click(await screen.findByRole("button", { name: /^Decide the rest per recommendation/ }));
    await screen.findByRole("heading", { name: /^Decide the rest per recommendation/ });
    expect(screen.queryByRole("checkbox", { name: "Include C21" })).toBeNull();
  });
});

describe("202 Accepted without a body (RC3 read again, RC6 assess again; review_loop.rs answers no body)", () => {
  it("Read GitHub again succeeds without an error", async () => {
    renderRoute(`${PR}/comments?item=C14`);
    await userEvent.click(await screen.findByRole("button", { name: "Questions waiting" }));
    await userEvent.click(screen.getByRole("button", { name: "Read GitHub again" }));
    await act(async () => {});
    expect(screen.queryByText("Something went wrong. Try again.")).toBeNull();
  });

  it("Assess again says it was asked", async () => {
    renderRoute(`${PR}/comments?item=C14`);
    await screen.findByRole("heading", { name: /C14 · Retry may resend/ });
    await userEvent.click(screen.getByRole("button", { name: "LLM assessment" }));
    await userEvent.click(screen.getByRole("button", { name: "Assess again" }));
    expect(
      await screen.findByText(/^Asked: diffpilot's LLM assesses this comment again\.$|Something went wrong/),
    ).toHaveTextContent("Asked: diffpilot's LLM assesses this comment again.");
  });
});

describe("Keyboard (plan §3.8): s skips a group question as the card says", () => {
  it("s on a group opens the next question", async () => {
    const router = renderRoute(`${PR}/comments?item=C18`);
    await screen.findByRole("heading", { name: /^Same recommendation/ });
    await userEvent.keyboard("s");
    await waitFor(() => expect(router.state.location.search).not.toBe("?item=C18"));
  });
});

describe("V0 PR page stays reachable (plan §3.1)", () => {
  it("opening the PR page of a PR whose latest analysis was cancelled keeps the PR page", async () => {
    setScenario("existing-cancelled");
    const router = renderRoute(PR);
    expect(await screen.findByText("Analysis cancelled.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Start analysis" })).toBeEnabled();
    expect(router.state.location.pathname).toBe(PR);
    expect(screen.getByRole("button", { name: "Process PR comments" })).toBeVisible();
  });
});
