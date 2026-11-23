import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it, vi } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { watches } from "../../mocks/review-loop";

const server = startMockServer();
const WATCH = "/api/v1/prs/:owner/:name/:number/watch";

async function form() {
  renderRoute("/watching");
  return screen.findByRole("textbox", { name: "GitHub pull request link" });
}

describe("Watch by pasted pull request link", () => {
  it("uses only RW1, refreshes RW2, announces success and keeps keyboard focus", async () => {
    let added = false;
    const mutations: string[] = [];
    const pr = { repo: { owner: "octo", name: "project" }, number: 123 };
    server.events.on("request:start", ({ request }) => {
      if (request.method !== "GET") mutations.push(`${request.method} ${new URL(request.url).pathname}`);
    });
    server.use(
      http.put(WATCH, ({ params }) => {
        expect(params).toMatchObject({ owner: "octo", name: "project", number: "123" });
        added = true;
        return new HttpResponse(null, { status: 204 });
      }),
      http.get("/api/v1/watches", () =>
        HttpResponse.json({
          ...watches(),
          watches: added ? [{ ...watches().watches[0]!, pr, title: "Pasted PR" }] : [],
        }),
      ),
    );
    const input = await form();
    await userEvent.type(input, "https://github.com/octo/project/pull/123/files/?diff=split#L7{Enter}");
    expect(await screen.findByRole("link", { name: "octo/project #123" })).toBeVisible();
    expect(screen.getByRole("status")).toHaveTextContent("Watching octo/project #123.");
    expect(input).toHaveValue("");
    expect(input).toHaveFocus();
    expect(mutations).toEqual(["PUT /api/v1/prs/octo/project/123/watch"]);
    server.events.removeAllListeners("request:start");
  });

  it.each([
    "https://github.com/acme/repo/issues/42",
    "https://github.com.evil.example/acme/repo/pull/42",
    "https://github.com@evil.example/acme/repo/pull/42",
    "https://user@github.com/acme/repo/pull/42",
    "http://github.com/acme/repo/pull/42",
    "https://github.com/acme/repo/pull/0",
    "https://github.com/acme/repo/pull/1e3",
    "https://github.com/acme/repo/pull/9007199254740992",
    "https://github.com/acme/repo/pull/42/commits",
    "not a link",
  ])("rejects a non-PR link without calling Watch: %s", async (url) => {
    const put = vi.fn(() => new HttpResponse(null, { status: 204 }));
    server.use(http.put(WATCH, put));
    const input = await form();
    await userEvent.type(input, `${url}{Enter}`);
    expect(screen.getByRole("alert")).toHaveTextContent("Enter a GitHub pull request link");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).toHaveAccessibleDescription(expect.stringContaining("Enter a GitHub pull request link"));
    expect(put).not.toHaveBeenCalled();
  });

  it("reports already watched, case-insensitively, without sending Watch", async () => {
    const put = vi.fn();
    server.use(http.put(WATCH, put));
    await userEvent.type(await form(), "https://github.com/ACME/Node-Client/pull/482#discussion{Enter}");
    expect(screen.getByRole("alert")).toHaveTextContent("already watched");
    expect(put).not.toHaveBeenCalled();
  });

  it.each([
    [
      422,
      "private_repository",
      "Private repositories are not supported. Watch reads public repositories only.",
    ],
    [409, "watch_limit_reached", "Watch limit reached (7 pull requests). Unwatch one before adding another."],
    [502, "github_unavailable", "GitHub did not answer. Try again."],
  ])("explains a %s %s response and allows retry", async (status, code, message) => {
    server.use(
      http.get("/api/v1/watches", () => HttpResponse.json({ ...watches(), limit: 7 })),
      http.put(WATCH, () => HttpResponse.json({ code, message: "GitHub did not answer." }, { status })),
    );
    const input = await form();
    await userEvent.type(input, "https://github.com/octo/project/pull/123{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    expect(input).toHaveValue("https://github.com/octo/project/pull/123");
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Watch pull request" })).not.toHaveAttribute("aria-disabled"),
    );
    server.use(http.put(WATCH, () => new HttpResponse(null, { status: 204 })));
    await userEvent.click(screen.getByRole("button", { name: "Watch pull request" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Watching octo/project #123."));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("keeps the pending control focusable with its tooltip and sends one request", async () => {
    let finish!: () => void;
    const completed = new Promise<void>((resolve) => {
      finish = resolve;
    });
    const put = vi.fn(async () => {
      await completed;
      return new HttpResponse(null, { status: 204 });
    });
    server.use(http.put(WATCH, put));
    const input = await form();
    await userEvent.type(input, "https://github.com/octo/project/pull/123{Enter}");
    const adding = await screen.findByRole("button", { name: "Adding…" });
    expect(adding).toHaveAttribute("aria-disabled", "true");
    expect(adding).toHaveAccessibleDescription(expect.stringContaining("Wait for it to finish"));
    expect(input).toHaveAttribute("readonly");
    await userEvent.click(adding);
    expect(put).toHaveBeenCalledTimes(1);
    finish();
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Watching octo/project #123."));
  });
});
