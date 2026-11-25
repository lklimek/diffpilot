import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import { commentItemsResponse, listItems, overview, prPage } from "../../mocks/review-loop";
import { setScenario } from "../../mocks/scenarios";

const server = startMockServer();
const PR = "/pr/acme/node-client/482";

/** Decide once the server planned the card's current input (until then Decide is unavailable). */
async function decideWhenPlanned() {
  const decide = screen.getByRole("button", { name: "Decide" });
  await waitFor(() => expect(decide).not.toHaveAttribute("aria-disabled"));
  await userEvent.click(decide);
}
const text = () => document.body.textContent ?? "";

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
  window.localStorage.clear();
});
afterEach(() => vi.unstubAllGlobals());

describe("V0 PR page (US1, US19)", () => {
  it("shows Review the code and Process PR comments; Watch toggles", async () => {
    renderRoute(PR);
    expect(await screen.findByRole("heading", { level: 1, name: /acme\/node-client #482/ })).toBeVisible();
    expect(screen.getByText(/^by octo-dev · base/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Review the code" })).toHaveAttribute("aria-expanded", "true");
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
    expect(screen.getByText(/Watching since/)).toBeVisible();
    expectTrustWording(text());
  });

  it("without an analysis: No analysis yet · Start analysis", async () => {
    setScenario("comments-no-analysis");
    renderRoute(PR);
    expect(await screen.findByText("No analysis yet.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Start analysis" })).toBeEnabled();
  });
});

describe("V1 Questions (US3, US4, US11)", () => {
  it("records one Comments entry before reading the overview and keeps it across tabs", async () => {
    let finishVisit!: () => void;
    const pending = new Promise<void>((resolve) => {
      finishVisit = resolve;
    });
    const visited = vi.fn();
    const readOverview = vi.fn();
    server.use(
      http.post("/api/v1/prs/acme/node-client/482/comments/visit", async ({ request }) => {
        visited(request.headers.get("X-CSRF-Token"));
        await pending;
        return new HttpResponse(null, { status: 204 });
      }),
      http.get("/api/v1/prs/acme/node-client/482/comments", () => {
        readOverview();
        return HttpResponse.json(overview());
      }),
    );
    renderRoute(PR);
    expect(visited).not.toHaveBeenCalled();
    await userEvent.click(await screen.findByRole("link", { name: "Open Comments →" }));
    try {
      await waitFor(() => expect(visited).toHaveBeenCalledOnce());
      expect(visited.mock.calls[0]?.[0]).toBeTruthy();
      expect(readOverview).not.toHaveBeenCalled();
    } finally {
      finishVisit();
    }
    await waitFor(() => expect(readOverview).toHaveBeenCalledOnce());
    await userEvent.click(await screen.findByRole("link", { name: "Table" }));
    expect(visited).toHaveBeenCalledTimes(1);
  });

  it("tries the Comments entry again after a transient failure and then reads the overview", async () => {
    let tries = 0;
    const readOverview = vi.fn();
    server.use(
      http.post("/api/v1/prs/acme/node-client/482/comments/visit", () => {
        tries += 1;
        return new HttpResponse(null, { status: tries === 1 ? 503 : 204 });
      }),
      http.get("/api/v1/prs/acme/node-client/482/comments", () => {
        readOverview();
        return HttpResponse.json(overview());
      }),
    );
    renderRoute(PR);
    await userEvent.click(await screen.findByRole("link", { name: "Open Comments →" }));
    expect(await screen.findByRole("navigation", { name: "Queue" })).toBeVisible();
    expect(readOverview).toHaveBeenCalledOnce();
    expect(tries).toBe(2);
  });

  it.each([200, 502])("reads items after the overview settles with HTTP %i", async (status) => {
    let finishOverview!: () => void;
    const pending = new Promise<void>((resolve) => {
      finishOverview = resolve;
    });
    const readOverview = vi.fn();
    const readItems = vi.fn();
    server.use(
      http.get("/api/v1/prs/acme/node-client/482/comments", async () => {
        readOverview();
        await pending;
        return status === 200
          ? HttpResponse.json(overview())
          : HttpResponse.json({ code: "github_unavailable", message: "Overview unavailable." }, { status });
      }),
      http.get("/api/v1/prs/acme/node-client/482/comments/items", () => {
        readItems();
        return HttpResponse.json(commentItemsResponse());
      }),
    );
    renderRoute(`${PR}/comments`);
    try {
      await waitFor(() => expect(readOverview).toHaveBeenCalledOnce());
      expect(readItems).not.toHaveBeenCalled();
    } finally {
      finishOverview();
    }
    await waitFor(() => expect(readItems).toHaveBeenCalledOnce());
    if (status === 200) expect(await screen.findByRole("navigation", { name: "Queue" })).toBeVisible();
    else expect(await screen.findByRole("alert")).toHaveTextContent("Overview unavailable.");
  });

  it("opens the first question of Needs you; PR-level questions come last; j moves to the next", async () => {
    const router = renderRoute(`${PR}/comments`);
    expect(await screen.findByRole("heading", { name: /C21 · Retry cap is checked/ })).toHaveFocus();
    expect(await screen.findByText("Why you are asked again")).toBeVisible();
    const queue = within(screen.getByRole("navigation", { name: "Queue" }));
    expect(queue.getByRole("button", { name: /^Status: Needs you · 11/ })).toBeVisible();
    expect(queue.getAllByRole("listitem").at(-1)).toHaveTextContent("P24");
    fireEvent.keyDown(document.body, { key: "j" });
    await waitFor(() => expect(router.state.location.search).toBe("?item=C12"));
  });

  it("?item=C14 opens it; a decision is stored and the next question opens with an announcement", async () => {
    renderRoute(`${PR}/comments?item=C14`);
    expect(await screen.findByRole("heading", { name: /C14 · Retry may resend/ })).toBeVisible();
    expect(await screen.findByText(/pub async fn send_with_retry/)).toBeVisible();
    await decideWhenPlanned();
    expect(await screen.findByText(/C14: decision recorded\. \d+ questions left\./)).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: /C15 · Debug log prints/ })).toBeVisible();
    await userEvent.click(screen.getByRole("link", { name: "Decision log and agents" }));
    expect(await screen.findByText(/C14 · Fix here/)).toBeVisible();
  });

  it("Decide on a question picked from the middle of the list opens the next one, not the first", async () => {
    renderRoute(`${PR}/comments`);
    const queue = await screen.findByRole("navigation", { name: "Queue" });
    await userEvent.click(await within(queue).findByRole("button", { name: /^C15(?!\d)/ }));
    expect(await screen.findByRole("heading", { name: /C15 · Debug log prints/ })).toBeVisible();
    await decideWhenPlanned();
    expect(
      await screen.findByRole("heading", { name: /C11 · Backoff cap is not configurable/ }),
    ).toBeVisible();
  });

  it("after a group the entry after it opens; after the last entry the first one still waiting", async () => {
    renderRoute(`${PR}/comments?item=C18`);
    expect(await screen.findByRole("heading", { name: /Same recommendation: Reject/ })).toBeVisible();
    const decideGroup = screen.getByRole("button", { name: /Reject all 2/ });
    await waitFor(() => expect(decideGroup).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(decideGroup);
    expect(await screen.findByRole("heading", { name: "Merge now, or another round?" })).toBeVisible();
    // P24 is the last entry; "Merge with follow-ups" hands the non-gate items to one follow-up PR.
    await decideWhenPlanned();
    expect(await screen.findByRole("heading", { name: /C14 · Retry may resend/ })).toBeVisible();
  });

  it("Undo takes a single decision back while no agent claimed it; the comment is asked again", async () => {
    const router = renderRoute(`${PR}/comments?item=C14`);
    expect(await screen.findByRole("heading", { name: /C14 · Retry may resend/ })).toBeVisible();
    await decideWhenPlanned();
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
    expect(screen.getByRole("menuitemradio", { name: /Flows/ })).toHaveTextContent("Needs an analysis");
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

  it("the watched filter opens in Pull requests and can stop a watch", async () => {
    renderRoute("/watching");
    expect(await screen.findByRole("heading", { name: "Pull requests" })).toBeVisible();
    expect(await screen.findByRole("button", { name: "Watched" })).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(
      await screen.findByRole("button", { name: "More actions for acme/node-client #482" }),
    );
    await userEvent.click(screen.getByRole("menuitem", { name: "Stop watching" }));
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "More actions for acme/node-client #482" })).toBeNull(),
    );
  });

  it("a failed read of the items keeps the page and what was typed, with the error beside it", async () => {
    renderRoute(`${PR}/comments/table`);
    const comment = await screen.findByLabelText("Your comment on C22");
    await userEvent.type(comment, "keep the test small");
    server.use(
      http.get("/api/v1/prs/:owner/:name/:number/comments/items", () =>
        HttpResponse.json(
          { code: "github_unavailable", message: "GitHub is not reachable." },
          { status: 502 },
        ),
      ),
    );
    await userEvent.click(screen.getByRole("button", { name: "Questions waiting" }));
    await userEvent.click(screen.getByRole("button", { name: "Read GitHub again" }));
    expect(await screen.findByText(/GitHub is not reachable\./)).toBeVisible();
    expect(screen.getByLabelText("Your comment on C22")).toHaveValue("keep the test small");
    server.resetHandlers();
    await userEvent.click(screen.getByRole("button", { name: "Read GitHub again" }));
    await waitFor(() => expect(screen.queryByText(/GitHub is not reachable\./)).toBeNull());
    expect(screen.getByLabelText("Your comment on C22")).toHaveValue("keep the test small");
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

  it("a new server address with the stored token kept is refused at the token field until it is replaced or removed", async () => {
    renderRoute("/settings#notifications");
    const ntfy = await screen.findByRole("region", { name: "ntfy" });
    const url = within(ntfy).getByLabelText("Server URL");
    await userEvent.clear(url);
    await userEvent.type(url, "https://push.example.org");
    await userEvent.click(within(ntfy).getByRole("button", { name: "Save channel" }));
    const message =
      "The kind or the URL's scheme, host or port changed: replace or remove the secret, it is not sent to a new destination.";
    expect(await within(ntfy).findByRole("alert")).toHaveTextContent(message);
    expect(within(ntfy).getByLabelText("Access token (optional)")).toHaveAccessibleDescription(
      expect.stringContaining(message),
    );
    expect(within(ntfy).getAllByText(message)).toHaveLength(1);
    await userEvent.click(within(ntfy).getByRole("button", { name: "Remove secret" }));
    await userEvent.click(within(ntfy).getByRole("button", { name: "Save channel" }));
    // A saved channel is shown afresh (the card is keyed by its stored URL and secret).
    await waitFor(() => {
      const saved = screen.getByRole("region", { name: "ntfy" });
      expect(within(saved).getByLabelText("Server URL")).toHaveValue("https://push.example.org");
      expect(within(saved).getByLabelText("Access token (optional)")).toHaveValue("not set");
      expect(within(saved).queryByRole("alert")).toBeNull();
    });
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

describe("v1.4 review status", () => {
  it("reports the same Needs you count in the PR card, Questions tab and queue", async () => {
    setScenario("comments-v14");
    server.use(
      http.get("/api/v1/prs/acme/node-client/482", () => {
        const page = prPage();
        return HttpResponse.json({
          ...page,
          comments: { ...page.comments, counts: { ...page.comments.counts, undecided: 99 } },
        });
      }),
      http.get("/api/v1/prs/acme/node-client/482/comments", () => {
        const data = overview();
        return HttpResponse.json({ ...data, counts: { ...data.counts, undecided: 98 } });
      }),
    );
    renderRoute(PR);
    const panel = await screen.findByRole("region", { name: "Process PR comments" });
    await waitFor(() =>
      expect(within(panel).getByText("Needs you").nextElementSibling).toHaveTextContent(/^11/),
    );
    await userEvent.click(within(panel).getByRole("link", { name: "Open Comments →" }));
    const queue = within(await screen.findByRole("navigation", { name: "Queue" }));
    expect(queue.getByRole("button", { name: /^Status: Needs you · 11/ })).toBeVisible();
    expect(screen.getByRole("link", { name: "Questions, 11 need you" })).toBeVisible();
    expect(
      within(screen.getByRole("complementary", { name: "PR panel" })).getByText("Needs you")
        .nextElementSibling,
    ).toHaveTextContent(/^11$/);
  });

  it("opens the PR status details and restores focus on Escape", async () => {
    renderRoute(PR);
    const trigger = await screen.findByRole("button", { name: "Questions waiting" });
    expect(screen.queryByText("PR Hygiene")).toBeNull();
    await userEvent.click(trigger);
    const panel = within(screen.getByRole("dialog", { name: "Pull request status" }));
    expect(panel.getByRole("heading", { name: /^PR Hygiene/ })).toBeVisible();
    expect(panel.getByText(/Bot rounds: 4/)).toBeVisible();
    await userEvent.tab();
    expect(panel.getAllByRole("link")[0]).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    expect(trigger).toHaveFocus();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
  });

  it("decides linked duplicate items with their primary and undoes the same decision together", async () => {
    setScenario("comments-v14");
    renderRoute(`${PR}/comments?item=C14`);
    const question = await screen.findByRole("article", { name: /C14 · Retry may resend/ });
    expect(within(question).getByText("Bot heading: Medium severity finding")).toBeVisible();
    expect(screen.getByRole("link", { name: /C30 \(coderabbit/ })).toBeVisible();
    await decideWhenPlanned();
    expect(await screen.findByText(/C14: decision recorded/)).toBeInTheDocument();
    for (const no of [30, 31])
      expect(
        listItems()
          .find((item) => item.no === no)
          ?.decisions.at(-1)?.kind,
      ).toBe("fix_here");
    await userEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(await screen.findByText(/C14: decision undone/)).toBeInTheDocument();
    for (const no of [30, 31])
      expect(
        listItems()
          .find((item) => item.no === no)
          ?.decisions.at(-1)?.undone_at,
      ).toBeTruthy();
  });
});

describe("reviewing duplicates and deferred questions", () => {
  it("shows a neutral assessment group when the server omits the current pass count", async () => {
    setScenario("comments-v14");
    server.use(
      http.get("/api/v1/prs/acme/node-client/482/comments", () => {
        const current = overview();
        return HttpResponse.json({
          ...current,
          assessment: { ...current.assessment, in_pass: undefined, waiting: undefined, since: undefined },
          queue: [],
        });
      }),
      http.get("/api/v1/prs/acme/node-client/482/comments/items", () =>
        HttpResponse.json(commentItemsResponse().filter((item) => item.no === 34)),
      ),
    );
    renderRoute(`${PR}/comments`);
    const summary = await screen.findByRole("region", { name: "No questions need you now" });
    expect(within(summary).getByRole("button", { name: "Pending or assessing · 1" })).toBeVisible();

    const queue = within(screen.getByRole("navigation", { name: "Queue" }));
    await userEvent.click(queue.getByRole("button", { name: "Status: Needs you · 0" }));
    const pending = queue.getByRole("menuitemradio", { name: "Pending or assessing · 1" });
    expect(pending).toHaveAccessibleDescription("Waiting for assessment or in an assessment pass.");
    await userEvent.click(pending);
    expect(queue.getByRole("button", { name: "Status: Pending or assessing · 1" })).toBeVisible();
    await userEvent.click(queue.getByRole("button", { name: /^C34 .*pending or assessing$/ }));
    expect(
      within(screen.getByRole("article", { name: "C34" })).getByText("◌ pending or assessing"),
    ).toBeVisible();
  });

  it("keeps legacy and gated duplicates individually decidable in Questions and Table", async () => {
    setScenario("comments-v14");
    const c30 = listItems().find((item) => item.no === 30)!;
    const c31 = listItems().find((item) => item.no === 31)!;
    Object.assign(c30, { decision_follows_primary: undefined });
    Object.assign(c31, {
      decision_follows_primary: true,
      gates: [{ gate: "G-FUNDS", source: "owner" }],
      state: "assessing",
      assessment_state: { state: "running" },
    });
    renderRoute(`${PR}/comments?item=C14`);
    const queue = within(await screen.findByRole("navigation", { name: "Queue" }));
    expect(queue.getByRole("button", { name: "Status: Needs you · 13, 3 gates" })).toBeVisible();
    expect(screen.queryByText(/Deciding C14 decides them too/)).toBeNull();
    await userEvent.click(queue.getByRole("button", { name: /^C30/ }));
    const question = await screen.findByRole("article", { name: /^C30 ·/ });
    expect(within(question).getByRole("button", { name: "Decide" })).toBeVisible();
    await userEvent.click(within(question).getByRole("link", { name: "Same point as C14" }));
    expect(await screen.findByRole("heading", { name: /^C14 ·/ })).toHaveFocus();
    await userEvent.click(screen.getByRole("link", { name: "Table" }));
    expect(await screen.findByRole("combobox", { name: /^Decision for C30/ })).toBeVisible();
    expect(screen.getByRole("combobox", { name: /^Decision for C31.*gate item/ })).toBeVisible();
  });

  it("shows counts and routes to pending views when nothing needs you now", async () => {
    setScenario("comments-v14");
    listItems().find((item) => item.no === 30)!.state = "assessing";
    listItems().find((item) => item.no === 37)!.assessment_state = {
      state: "not_assessed",
      reason: "missing",
    };
    server.use(
      http.get("/api/v1/prs/acme/node-client/482/comments", () =>
        HttpResponse.json({ ...overview(), queue: [] }),
      ),
      http.get("/api/v1/prs/acme/node-client/482/comments/items", () =>
        HttpResponse.json(
          commentItemsResponse().filter((item) => [30, 31, 34, 35, 36, 37].includes(item.no)),
        ),
      ),
    );
    renderRoute(`${PR}/comments`);
    const summary = await screen.findByRole("region", { name: "No questions need you now" });
    expect(within(summary).getByRole("button", { name: "Assessing now · 1" })).toBeVisible();
    expect(within(summary).getByRole("button", { name: "Waiting for assessment · 1" })).toBeVisible();
    expect(within(summary).getByRole("button", { name: "Duplicates · 2" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "No undecided questions" })).toBeNull();
    await userEvent.click(within(summary).getByRole("button", { name: "Not assessed · 2" }));
    const queue = within(screen.getByRole("navigation", { name: "Queue" }));
    expect(queue.getByRole("button", { name: /^C37/ })).toHaveTextContent(
      "Not assessed: left out of the pass",
    );
  });

  it.each([
    ["table", "Table"],
    ["log", "Decision log and agents"],
    ["carry-overs", "Carry-overs"],
  ])("hides assessment without leaving the %s tab", async (path, tab) => {
    setScenario("comments-v14");
    const router = renderRoute(`${PR}/comments/${path}`);
    const note = path === "table" ? await screen.findByLabelText("Your comment on C22") : null;
    if (note) await userEvent.type(note, "Keep this draft");
    await userEvent.click(await screen.findByRole("button", { name: "Hide" }));
    expect(router.state.location.pathname).toBe(`${PR}/comments/${path}`);
    expect(screen.getByRole("link", { name: tab })).toHaveFocus();
    if (note) expect(screen.getByLabelText("Your comment on C22")).toHaveValue("Keep this draft");
  });
});
