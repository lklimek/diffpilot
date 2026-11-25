// Review repros for the floating Ask chat: chip links, anchors, drafts, sign-in and refusal texts.
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import type { AnalysisSummary, AnchoredAskRequest, DiffResponse } from "../../api/generated/types";
import { seedFor, setFixtureState } from "../../mocks/fixtures";

const server = startMockServer();
const ID = (seedFor("default", "analysis") as AnalysisSummary).id;

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => {
  vi.unstubAllGlobals();
  window.localStorage.clear();
});

const user = () => userEvent.setup();
const chat = () => screen.getByRole("dialog", { name: "Ask" });
const field = () => within(chat()).getByRole("textbox", { name: "Your question" });

async function openTour(hash = "") {
  const router = renderRoute(`/analyses/${ID}${hash}`);
  await within(await screen.findByRole("navigation", { name: "Flows and steps" })).findByRole("button", {
    name: /^Flow 1:/,
  });
  return router;
}

function captureAsk() {
  const bodies: AnchoredAskRequest[] = [];
  const record = ({ request }: { request: Request }) => {
    if (request.method === "POST" && /\/analyses\/[^/]+\/questions$/.test(request.url))
      void request
        .clone()
        .json()
        .then((body: AnchoredAskRequest) => bodies.push(body));
  };
  server.events.on("request:start", record);
  return { bodies, stop: () => server.events.removeListener("request:start", record) };
}

describe("Ask chat review repros", () => {
  it("a thread chip with selected lines opens those lines in Files (AC7)", async () => {
    const reader = user();
    const router = renderRoute(
      `/analyses/${ID}/files#lines?file=src/rpc/client.rs&side=head&line=100&end=102`,
    );
    await screen.findByRole("button", { name: /^Ask.*: open your private chat/ });
    await screen.findByRole("heading", { name: "Your selection · client.rs lines 100–102" });
    await reader.keyboard("a");
    await waitFor(() => expect(chat()).toHaveTextContent(/About:\s*Step 1\.6 · client\.rs 100–102/));
    await waitFor(() => expect(field()).toHaveFocus());
    await reader.paste("Why the delay?");
    await reader.keyboard("{Enter}");
    const thread = await waitFor(() => {
      const t = chat().querySelector<HTMLElement>("[data-thread]");
      expect(t).not.toBeNull();
      return t!;
    });
    const chip = within(thread).getByRole("button", { name: /client\.rs 100–102/ });
    expect(chip).toHaveAccessibleDescription(/Opens them in the Files view/);
    // Navigate away first so the click must bring the reader back.
    await act(() => router.navigate(`/analyses/${ID}#pr_context`));
    await reader.click(chip);
    expect(router.state.location.pathname).toBe(`/analyses/${ID}/files`);
    expect(decodeURIComponent(router.state.location.hash)).toContain("line=100&end=102");
  }, 10_000);

  it("asking about a classic-diff context line outside the step sends a lines anchor (server refuses step lines outside the step)", async () => {
    // The real server's classic diff carries three context lines around the step (http-api.md "Classic diff").
    server.use(
      http.get("/api/v1/analyses/:id/diff", () =>
        HttpResponse.json({
          path: "src/rpc/client.rs",
          side: "head",
          hunks: [
            {
              header: "@@ -97,2 +97,3 @@",
              lines: [
                { kind: "context", old_line: 97, new_line: 97, text: "    match x {", eol: null },
                {
                  kind: "context",
                  old_line: 98,
                  new_line: 98,
                  text: "    Ok(r) => return Ok(r),",
                  eol: null,
                },
                {
                  kind: "added",
                  old_line: null,
                  new_line: 99,
                  text: "    Err(RpcError::Timeout) if policy.should_retry(attempt) => {",
                  eol: null,
                },
              ],
            },
          ],
        } satisfies DiffResponse),
      ),
    );
    const seen = captureAsk();
    try {
      await openTour("#f1.5");
      const inspector = screen.getByRole("complementary", { name: "Inspector" });
      const diff = await within(inspector).findByRole("region", {
        name: "Classic diff of src/rpc/client.rs",
      });
      await user().click(await within(diff).findByRole("button", { name: "Ask the LLM about line 98" }));
      await user().click(within(diff).getByRole("button", { name: "Ask about this line" }));
      await user().type(field(), "Is this reached?{Enter}");
      await waitFor(() => expect(seen.bodies).toHaveLength(1));
      // Step f1.5 covers line 99 only: a step anchor with lines 98 is a 400 invalid_request on the real server.
      expect(seen.bodies[0]!.anchor).toMatchObject({
        kind: "lines",
        path: "src/rpc/client.rs",
        side: "head",
        range: { start_line: 98, end_line: 98 },
      });
    } finally {
      seen.stop();
    }
  });

  it("a follow-up draft survives Hide and reopening (AC4: follow-up drafts are kept)", async () => {
    setFixtureState("resume");
    await openTour("#f1.5");
    await user().keyboard("a");
    const thread = await waitFor(() => {
      const t = chat().querySelector<HTMLElement>("[data-thread]");
      expect(t).not.toBeNull();
      return t!;
    });
    await user().click(within(thread).getByRole("button", { name: "Follow up on Q1" }));
    await user().type(within(thread).getByRole("textbox", { name: /^Follow-up to Q1/ }), "half");
    await user().click(within(chat()).getByRole("button", { name: "Hide the Ask chat" }));
    await user().click(screen.getByRole("button", { name: /^Ask.*: open your private chat/ }));
    const again = chat().querySelector<HTMLElement>("[data-thread]")!;
    const reply = within(again).queryByRole("textbox", { name: /^Follow-up to Q1/ });
    if (reply) expect(reply).toHaveValue("half");
    else {
      await user().click(within(again).getByRole("button", { name: "Follow up on Q1" }));
      expect(within(again).getByRole("textbox", { name: /^Follow-up to Q1/ })).toHaveValue("half");
    }
  });

  it("a 401 on send offers the Sign in again path and keeps the text (ST2)", async () => {
    server.use(
      http.post("/api/v1/analyses/:id/questions", () =>
        HttpResponse.json({ code: "unauthenticated", message: "Sign in to continue." }, { status: 401 }),
      ),
    );
    await openTour("#f1.5");
    await user().keyboard("a");
    await user().type(field(), "Why?{Enter}");
    expect(await within(chat()).findByRole("alert")).toHaveTextContent(/Sign in/);
    expect(field()).toHaveValue("Why?");
    expect(within(chat()).getByRole("link", { name: /Sign in/ })).toBeVisible();
  });
  it.each([
    [
      "503 with the server's own text",
      () =>
        HttpResponse.json(
          { code: "unavailable", message: "The server is busy. Try again in a moment." },
          { status: 503 },
        ),
    ],
    [
      "500 with the server's own text",
      () =>
        HttpResponse.json(
          { code: "internal", message: "Something went wrong on the server. Try again later." },
          { status: 500 },
        ),
    ],
    ["a lost connection", () => HttpResponse.error()],
  ])("a send failure (%s) says when to try again once (ST1)", async (_name, reply) => {
    server.use(http.post("/api/v1/analyses/:id/questions", reply));
    await openTour("#f1.5");
    await user().keyboard("a");
    await user().type(field(), "Why?{Enter}");
    const alert = await within(chat()).findByRole("alert");
    expect(alert.textContent!.match(/try again|retry/gi)).toHaveLength(1);
  });
});
