import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";

const server = startMockServer();

it("merges analyses and watches while history is reserved", async () => {
  server.use(http.get("/api/v1/me/prs", () => new HttpResponse(null, { status: 501 })));
  renderRoute("/prs");
  const list = await screen.findByRole("list", { name: "Your pull requests" });
  await within(list).findByText(/analysis .*failed/);
  const matches = within(list)
    .getAllByRole("listitem")
    .filter((node) => node.textContent?.includes("acme/node-client #455"));
  expect(matches).toHaveLength(1);
  const shared = matches[0]!;
  expect(shared).toHaveTextContent("failed");
  expect(shared).toHaveTextContent("watch ended");
  expect(within(shared).queryByRole("link", { name: /^Review\b/ })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Watched" }));
  expect(within(list).queryByText(/wallet-ui/)).toBeNull();
});

it("redirects an invalid legacy link with the original value and inline guidance", async () => {
  const router = renderRoute("/review?pr=not-a-pr");
  expect(await screen.findByLabelText("GitHub pull request URL")).toHaveValue("not-a-pr");
  expect(screen.getByRole("alert")).toHaveTextContent("owner/repo#123");
  expect(router.state.location.pathname).toBe("/prs");
});

it("opens a shorthand reference on the PR page and starts code analysis only by explicit choice", async () => {
  let starts = 0;
  let assessed = 0;
  server.use(
    http.get("/api/v1/prs/acme/node-client/482/comments", () => {
      assessed += 1;
      return new HttpResponse(null, { status: 500 });
    }),
    http.post("/api/v1/analyses", () => {
      starts += 1;
      return new HttpResponse(null, { status: 500 });
    }),
  );
  const router = renderRoute("/prs");
  await userEvent.type(await screen.findByLabelText("GitHub pull request URL"), "acme/node-client#482");
  await userEvent.click(screen.getByRole("button", { name: "Open" }));
  const panel = await screen.findByRole("region", { name: "Review the code" });
  const start = await within(panel).findByRole("button", { name: "Start analysis" });
  expect(router.state.location.pathname).toBe("/pr/acme/node-client/482");
  expect(starts).toBe(0);
  expect(assessed).toBe(0);
  await userEvent.click(start);
  await waitFor(() => expect(starts).toBe(1));
});

it("loads the next C17 page and keeps opened-only PRs without a review action", async () => {
  const first = Array.from({ length: 50 }, (_, index) => ({
    pr: { repo: { owner: "acme", name: "project" }, number: index + 2 },
    title: `Example ${index + 2}`,
    last_activity_at: "2026-11-25T10:00:00Z",
    opened_at: "2026-11-25T10:00:00Z",
  }));
  server.use(
    http.get("/api/v1/me/prs", ({ request }) => {
      const query = new URL(request.url).searchParams;
      if (query.get("cursor") === "next") {
        return HttpResponse.json({
          prs: [
            {
              ...first[0],
              pr: { repo: { owner: "acme", name: "project" }, number: 1 },
              title: "Older visit",
            },
          ],
          next_cursor: null,
        });
      }
      expect(query.get("limit")).toBe("50");
      return HttpResponse.json({ prs: first, next_cursor: "next" });
    }),
  );
  renderRoute("/prs");
  const list = await screen.findByRole("list", { name: "Your pull requests" });
  expect(within(list).getAllByRole("listitem")).toHaveLength(50);
  expect(within(list).queryByRole("link", { name: /^Review\b/ })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Show more" }));
  expect(await within(list).findByText(/Older visit/)).toBeVisible();
  expect(within(list).getAllByRole("listitem")).toHaveLength(51);
});

it("counts active watches in the usage header, excluding the ended watch from the numerator", async () => {
  server.use(http.get("/api/v1/me/prs", () => new HttpResponse(null, { status: 501 })));
  renderRoute("/prs");
  expect(await screen.findByText("Watching 4 of 20")).toBeVisible();
  expect(await screen.findByText(/^79 of 80 daily passes remaining · resets at /)).toBeVisible();
  expect(await screen.findByText("ended: merged or closed")).toBeVisible();
});

it.each([404, 405])("surfaces HTTP %i as a failed history request", async (status) => {
  server.use(
    http.get("/api/v1/me/prs", () =>
      HttpResponse.json({ code: "not_found", message: "History could not be read." }, { status }),
    ),
  );
  renderRoute("/prs");
  expect(await screen.findByRole("alert")).toHaveTextContent("History could not be read.");
});

it("shows a plain message when the history response cannot be parsed", async () => {
  server.use(http.get("/api/v1/me/prs", () => HttpResponse.html("<p>Broken upstream response</p>")));
  renderRoute("/prs");
  expect(await screen.findByRole("alert")).toHaveTextContent("Something went wrong. Try again.");
});

it("names review links and shows the request time separately from the latest opening", async () => {
  server.use(
    http.get("/api/v1/me/prs", () =>
      HttpResponse.json({
        prs: [482, 483].map((number) => ({
          pr: { repo: { owner: "acme", name: "node-client" }, number },
          title: `PR ${number}`,
          last_activity_at: "2026-11-25T10:00:00Z",
          opened_at: "2026-11-25T10:00:00Z",
          analysis: {
            id: `analysis-${number}`,
            status: "finished",
            head_sha: "abc123",
            head_moved: false,
            requested_at: "2026-11-24T18:00:00Z",
          },
        })),
        next_cursor: null,
      }),
    ),
  );
  renderRoute("/prs");
  const list = within(await screen.findByRole("list", { name: "Your pull requests" }));
  for (const request of list.getAllByText(/^analysis requested$/)) {
    expect(request.querySelector("time")).toHaveAttribute("dateTime", "2026-11-24T18:00:00Z");
  }
  expect(list.getByRole("link", { name: "Review acme/node-client #482" })).toHaveAttribute(
    "href",
    "/analyses/analysis-482",
  );
  expect(list.getByRole("link", { name: "Review acme/node-client #483" })).toHaveAttribute(
    "href",
    "/analyses/analysis-483",
  );
});

it("discloses automatic assessment and token use before starting a watch from history", async () => {
  server.use(http.get("/api/v1/me/prs", () => new HttpResponse(null, { status: 501 })));
  renderRoute("/prs");
  await userEvent.click(await screen.findByRole("button", { name: "More actions for acme/wallet-ui #96" }));
  expect(screen.getByRole("menuitem", { name: "Watch" })).toHaveAccessibleDescription(
    /assessed automatically.*uses LLM tokens/i,
  );
});
