import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import { listItems, overview } from "../../mocks/review-loop";
import { setScenario } from "../../mocks/scenarios";

startMockServer();
const PR = "/pr/acme/node-client/482";
const text = () => document.body.textContent ?? "";

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
  window.localStorage.clear();
});
afterEach(() => vi.unstubAllGlobals());

describe("V0 PR page (US1, US19)", () => {
  it("shows Review a PR and Process PR comments; Watch toggles", async () => {
    renderRoute(PR);
    expect(await screen.findByRole("heading", { level: 1, name: /acme\/node-client #482/ })).toBeVisible();
    expect(screen.getByText(/^by octo-dev · base/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Review a PR" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "Process PR comments" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByText("Unresolved threads on GitHub")).toBeVisible();
    expect(screen.getByRole("link", { name: "Open Comments →" })).toHaveAttribute("href", `${PR}/comments`);
    expect(
      screen.getByRole("link", { name: "Open acme/node-client #482 on GitHub (new tab)" }),
    ).toHaveAttribute("target", "_blank");
    await userEvent.click(screen.getByRole("button", { name: "Watch" }));
    expect(await screen.findByRole("button", { name: "Watching" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(/new comments are assessed automatically and use LLM tokens/)).toBeVisible();
    expectTrustWording(text());
  });

  it("without an analysis: No analysis yet · Check and start", async () => {
    setScenario("comments-no-analysis");
    renderRoute(PR);
    expect(await screen.findByText("No analysis yet.")).toBeVisible();
    expect(screen.getByRole("link", { name: "Check and start" })).toHaveAttribute(
      "href",
      `/review?pr=${encodeURIComponent("https://github.com/acme/node-client/pull/482")}`,
    );
  });
});

describe("V1 Questions (US3, US4, US11)", () => {
  it("opens the PR-level question first; j moves to the next question", async () => {
    const router = renderRoute(`${PR}/comments`);
    expect(await screen.findByRole("heading", { name: "Merge now, or another round?" })).toHaveFocus();
    expect(screen.getByRole("navigation", { name: "Queue" })).toHaveTextContent("PR-level (1)");
    fireEvent.keyDown(document.body, { key: "j" });
    await waitFor(() => expect(router.state.location.search).toBe("?item=C21"));
    expect(await screen.findByText("Why you are asked again")).toBeVisible();
  });

  it("?item=C14 opens it; a decision is stored and the next question opens with an announcement", async () => {
    renderRoute(`${PR}/comments?item=C14`);
    expect(await screen.findByRole("heading", { name: /C14 · Retry may resend/ })).toBeVisible();
    expect(await screen.findByText(/pub async fn send_with_retry/)).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(await screen.findByText(/C14: decision recorded\. \d+ questions left\./)).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Merge now, or another round?" })).toBeVisible();
    await userEvent.click(screen.getByRole("link", { name: "Decision log and agents" }));
    expect(await screen.findByText(/C14 · Fix here/)).toBeVisible();
  });

  it("Undo takes a single decision back while no agent claimed it; the comment is asked again", async () => {
    const router = renderRoute(`${PR}/comments?item=C14`);
    expect(await screen.findByRole("heading", { name: /C14 · Retry may resend/ })).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    await screen.findByText(/C14: decision recorded/);
    await userEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(await screen.findByText("C14: decision undone; it is asked again.")).toBeInTheDocument();
    await waitFor(() => expect(router.state.location.search).toBe("?item=C14"));
    const c14 = listItems().find((i) => i.no === 14)!;
    expect(c14.state).toBe("undecided");
    expect(c14.decisions.at(-1)?.undone_at).toBeTruthy();
    expect(c14.decisions.at(-1)?.effects.every((e) => e.state === "cancelled")).toBe(true);
  });

  it("queue rows open their comment on GitHub in a new tab (plan §3.9)", async () => {
    renderRoute(`${PR}/comments?item=C14`);
    const queue = await screen.findByRole("navigation", { name: "Queue" });
    expect(within(queue).getByRole("link", { name: "Open C14 on GitHub (new tab)" })).toHaveAttribute(
      "target",
      "_blank",
    );
  });

  it("Skip for now puts the question last for this reader on the server too", async () => {
    renderRoute(`${PR}/comments?item=C14`);
    expect(await screen.findByRole("heading", { name: /C14 · Retry may resend/ })).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: /Skip for now/ }));
    await waitFor(() => expect(overview().queue.at(-1)).toMatchObject({ item_nos: [14], skipped: true }));
  });

  it("a group can be split; Decide the rest leaves gate items out", async () => {
    renderRoute(`${PR}/comments?item=C18`);
    expect(await screen.findByRole("heading", { name: /Same recommendation: Reject/ })).toBeVisible();
    await userEvent.click(await screen.findByRole("button", { name: /Decide the rest per recommendation/ }));
    expect(
      await screen.findByRole("heading", { name: /Decide the rest per recommendation — confirm/ }),
    ).toBeVisible();
    expect(screen.getByRole("alert")).toHaveTextContent(
      /C14 \(gate item G-FUNDS\), C15 \(gate item G-SECRET\)/,
    );
  });

  it("without an analysis Ask still answers about the comment; the menu views need one", async () => {
    setScenario("comments-no-analysis");
    renderRoute(`${PR}/comments?item=C11`);
    expect(await screen.findByRole("heading", { name: /C11 · Backoff cap/ })).toBeVisible();
    const ask = screen.getByRole("button", { name: "Ask about this comment" });
    expect(ask).not.toHaveAttribute("aria-disabled");
    await userEvent.click(ask);
    const chat = document.querySelector("[data-ask-chat]") as HTMLElement;
    expect(within(chat).getByText("Comment C11")).toBeVisible();
    await userEvent.type(
      within(chat).getByRole("textbox", { name: "Your question" }),
      "Why out of scope?{Enter}",
    );
    expect(await within(chat).findByText("Why out of scope?")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "View: Comments" }));
    expect(screen.getByRole("link", { name: /Flows/ })).toHaveTextContent("Needs an analysis");
  });

  it("GitHub read failure: the last read and the failure in plain words", async () => {
    setScenario("comments-github-error");
    renderRoute(`${PR}/comments`);
    expect(await screen.findByText("GitHub could not be read (rate limit).")).toBeVisible();
    expect(screen.getByText(/Decisions on these items still work/)).toBeVisible();
  });

  it("empty queue says what was read", async () => {
    setScenario("comments-empty");
    renderRoute(`${PR}/comments`);
    expect(await screen.findByRole("heading", { name: "No undecided questions" })).toBeVisible();
    expectTrustWording(text());
  });

  it("Ask about this comment opens the shared chat with the subject Comment C14", async () => {
    renderRoute(`${PR}/comments?item=C14`);
    await screen.findByRole("heading", { name: /C14 · Retry may resend/ });
    await userEvent.click(screen.getByRole("button", { name: "Ask about this comment" }));
    const chat = await screen
      .findByRole("region", { name: /Ask/ })
      .catch(() => document.querySelector("[data-ask-chat]") as HTMLElement);
    expect(within(chat).getByText("Comment C14")).toBeVisible();
  });
});

describe("V2 Table page (US8, US27)", () => {
  it("the problem opens Questions with that item", async () => {
    const router = renderRoute(`${PR}/comments/table`);
    await userEvent.click(await screen.findByRole("link", { name: "Jitter uses a fixed seed" }));
    await waitFor(() => expect(router.state.location.search).toBe("?item=C22"));
    expect(await screen.findByRole("heading", { name: /C22 · Jitter uses a fixed seed/ })).toBeVisible();
  });
});

describe("V4–V6 and Settings → Notifications", () => {
  it("Reopen on a carried repeat asks it again and opens its question", async () => {
    const router = renderRoute(`${PR}/comments/carry-overs`);
    const carried = await screen.findAllByRole("button", { name: "Reopen" });
    const row = carried[0]!.closest("tr")!;
    const no = Number(/C(\d+)/.exec(row.textContent ?? "")![1]);
    await userEvent.click(carried[0]!);
    await waitFor(() => expect(router.state.location.search).toBe(`?item=C${no}`));
    expect(listItems().find((i) => i.no === no)?.state).toBe("reopened");
  });

  it("rounds, carry-overs (this PR and all), watched list", async () => {
    renderRoute(`${PR}/comments/rounds`);
    expect(await screen.findByRole("table", { name: "Bot rounds of #482" })).toBeVisible();
    await userEvent.click(screen.getByRole("link", { name: "Carry-overs" }));
    await userEvent.click(await screen.findByRole("link", { name: "All pull requests" }));
    expect(await screen.findByRole("heading", { name: "Carry-overs on all pull requests" })).toBeVisible();
  });

  it("watched list unwatches", async () => {
    renderRoute("/watching");
    expect(await screen.findByRole("heading", { name: "Watched pull requests" })).toBeVisible();
    expect(screen.getByRole("link", { name: /Watched PRs/ })).toHaveAttribute("aria-current", "page");
    await userEvent.click(await screen.findByRole("button", { name: "Unwatch acme/node-client #482" }));
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Unwatch acme/node-client #482" })).toBeNull(),
    );
  });

  it("a refused save's error goes once a later Send test of the channel succeeds", async () => {
    renderRoute("/settings#notifications");
    const ntfy = await screen.findByRole("region", { name: "ntfy" });
    const url = within(ntfy).getByLabelText("Server URL");
    await userEvent.clear(url);
    await userEvent.type(url, "ftp://ntfy.example.lan");
    await userEvent.click(within(ntfy).getByRole("button", { name: "Save channel" }));
    expect(await within(ntfy).findByText("Only http and https addresses are allowed.")).toBeVisible();
    await userEvent.click(within(ntfy).getByRole("button", { name: "Send test" }));
    expect(await within(ntfy).findByText(/Test delivered/)).toBeVisible();
    expect(within(ntfy).queryByText("Only http and https addresses are allowed.")).toBeNull();
  });

  it("channels with masked tokens; Send test shows its outcome", async () => {
    renderRoute("/settings#notifications");
    expect(await screen.findByRole("heading", { name: "ntfy" })).toBeVisible();
    expect(screen.getByLabelText("Access token (optional)")).toHaveValue("•••• 9f2a");
    const webhook = screen.getByRole("region", { name: /Webhook/ });
    await userEvent.click(within(webhook).getByRole("button", { name: "Send test" }));
    expect(await within(webhook).findByText(/Test failed: connection refused/)).toBeVisible();
    await act(async () => {});
  });
});
