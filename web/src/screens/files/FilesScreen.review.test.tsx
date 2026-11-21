// Contract checks of the Files view from the independent review (proposal §3, §4, §6; http-api.md R14).
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import type { AnalysisSummary, AnchoredAskRequest } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";

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
const inspector = () => screen.getByRole("complementary", { name: "Inspector" });
const askChat = () => screen.getByRole("dialog", { name: "Ask" });
const askField = () => within(askChat()).getByRole("textbox", { name: "Your question" });
const inspectorHeading = () =>
  within(inspector())
    .getAllByRole("heading", { level: 2 })
    .find((h) => !h.hasAttribute("data-column-title"))!;
const fileHeading = () =>
  within(screen.getByRole("main"))
    .getAllByRole("heading", { level: 2 })
    .find((h) => !h.hasAttribute("data-column-title"))!;

async function open(hash: string) {
  const router = renderRoute(`/analyses/${ID}/files${hash}`);
  await waitFor(() => expect(fileHeading()).toBeDefined());
  return router;
}

function captureAsk(): { body: AnchoredAskRequest | null } {
  const seen: { body: AnchoredAskRequest | null } = { body: null };
  server.use(
    http.post("/api/v1/analyses/:id/questions", async ({ request }) => {
      seen.body = (await request.json()) as AnchoredAskRequest;
      return HttpResponse.json({}, { status: 500 });
    }),
  );
  return seen;
}

describe("Files view review checks", () => {
  // R14: a step anchor's `lines` are numbers of the step's side. Removed (base) line 41 of client.rs belongs to
  // head-side step f1.9 (head lines 30–43); head line 41 is other code.
  it("a question about removed lines is never a step anchor on the step's (head) numbers", async () => {
    const seen = captureAsk();
    await open("#lines?file=src%2Frpc%2Fclient.rs&side=base&line=41");
    await user().click(within(inspector()).getByRole("button", { name: "Ask about these lines" }));
    await user().type(askField(), "Q?{Enter}");
    await waitFor(() => expect(seen.body).not.toBeNull());
    expect(seen.body!.anchor).toEqual({
      kind: "lines",
      path: "src/rpc/client.rs",
      side: "base",
      range: { start_line: 41, end_line: 41 },
    });
  });

  // Help: "close: step details (back to the hunk), then the selection (back to the file)"; US10: Esc clears it.
  it("Esc after selecting a line by its number goes back to the file", async () => {
    await open("#lines?file=src%2Frpc%2Fclient.rs&side=head");
    await user().click(
      await within(screen.getByRole("main")).findByRole("button", { name: "Select line 100" }),
    );
    expect(inspectorHeading()).toHaveTextContent("Your selection · client.rs line 100");
    await user().keyboard("{Escape}");
    expect(inspectorHeading()).toHaveTextContent("src/rpc/client.rs");
  });

  // Proposal §3 / US2: "Given a hunk with one step, then that step's sections follow directly."
  it("a hunk with exactly one step shows that step's sections directly", async () => {
    await open("#lines?file=src%2Fconfig.rs&side=head&line=23");
    expect(inspectorHeading()).toHaveTextContent("config.rs · hunk");
    expect(within(inspector()).getByRole("region", { name: /Assessment/ })).toBeDefined();
  });
});
