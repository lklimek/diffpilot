import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";

const server = startMockServer();

it.each([404, 501])("merges legacy analyses and watches when history returns %s", async (status) => {
  server.use(http.get("/api/v1/me/prs", () => new HttpResponse(null, { status })));
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
  expect(within(shared).queryByRole("link", { name: "Review" })).toBeNull();
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
  expect(within(list).queryByRole("link", { name: "Review" })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Show more" }));
  expect(await within(list).findByText(/Older visit/)).toBeVisible();
  expect(within(list).getAllByRole("listitem")).toHaveLength(51);
});
