import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { listItems } from "../../mocks/review-loop";

const server = startMockServer();
const PR = "/pr/acme/node-client/482";

const failures = [
  {
    status: 429,
    body: { code: "assessment_daily_limit", resets_at: "2026-11-26T00:00:00Z" },
    note: /Daily comment assessment limit reached/,
  },
  {
    status: 409,
    body: { code: "credential_missing" },
    note: /Link a Codex account in Settings/,
  },
];

function expectAdmissionNote(note: RegExp, resetsAt?: string) {
  const message = screen.getByText(note);
  const status = message.closest('[role="status"]');
  expect(status).toBeVisible();
  if (resetsAt) expect(status?.querySelector("time")).toHaveAttribute("datetime", resetsAt);
  else expect(within(status as HTMLElement).getByRole("link")).toHaveAttribute("href", "/settings/account");
  expect(screen.queryByRole("alert")).toBeNull();
}

it.each(failures)("explains a $status assessment refusal on entry", async ({ status, body, note }) => {
  server.use(
    http.post("/api/v1/prs/:owner/:name/:number/comments/assess", () => HttpResponse.json(body, { status })),
  );
  renderRoute(`${PR}/comments`);
  await screen.findByRole("navigation", { name: "Queue" });
  expectAdmissionNote(note, body.resets_at);
});

it.each(failures)(
  "keeps a successful GitHub read when assessment returns $status",
  async ({ status, body, note }) => {
    let starts = 0;
    const title = "A newly read discussion";
    server.use(
      http.post("/api/v1/prs/:owner/:name/:number/comments/assess", () =>
        ++starts === 1 ? new HttpResponse(null, { status: 202 }) : HttpResponse.json(body, { status }),
      ),
      http.post("/api/v1/prs/:owner/:name/:number/comments/read", () => {
        const items = listItems();
        items.push({
          ...items.find(({ no }) => no === 14)!,
          no: 90,
          id: "new-item",
          title,
          llm_title: undefined,
        });
        return new HttpResponse(null, { status: 202 });
      }),
    );
    renderRoute(`${PR}/comments`);
    await screen.findByRole("navigation", { name: "Queue" });
    await userEvent.click(screen.getByRole("button", { name: "Questions waiting" }));
    await userEvent.click(screen.getByRole("button", { name: "Read GitHub again" }));
    expect(await screen.findByRole("button", { name: /C90.*A newly read discussion/ })).toBeVisible();
    await screen.findByRole("button", { name: "Read GitHub again" });
    expectAdmissionNote(note, body.resets_at);
    expect(starts).toBe(2);
  },
);
