import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { expect, it } from "vitest";
import items from "../../../../fixtures/dashboard/api/items.json";
import summary from "../../../../fixtures/dashboard/api/summary.json";
import { renderRoute, startMockServer } from "../../__tests__/screen";

const server = startMockServer();

it("leaves known-private PRs in secondary lists linked only to GitHub", async () => {
  const mention = {
    ...summary.mentions[0]!,
    kind: "pr",
    title: "Private PR mention",
    number: 333,
    url: "https://github.com/acme/wallet-ui/pull/333#issuecomment-1",
  };
  server.use(
    http.get("/api/v1/dashboard", () => HttpResponse.json({ ...summary, mentions: [mention] })),
    http.get("/api/v1/dashboard/items", () =>
      HttpResponse.json({
        ...items,
        items: items.items.map((item) =>
          item.repo && ["wallet-ui", "platform"].includes(item.repo.name)
            ? { ...item, repo: { ...item.repo, owner: "ACME" }, private: true }
            : item,
        ),
      }),
    ),
  );

  renderRoute("/");
  await screen.findByRole("region", { name: "Needs you" });
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Mentions (1)" }));
  await user.click(screen.getByRole("button", { name: /^Status:/ }));
  await user.click(screen.getByRole("menuitemradio", { name: /^Excluded and snoozed/ }));

  const privateRows = [
    [
      "feat(wallet): show pending withdrawals",
      "Open WUI-311 on GitHub (new tab)",
      "https://github.com/acme/wallet-ui/pull/311",
    ],
    [
      "feat(sync): parallel header download",
      "Open PLAT-1176 on GitHub (new tab)",
      "https://github.com/acme/platform/pull/1176",
    ],
    [
      "chore(deps): bump vite from 7.1.2 to 7.1.3",
      "Open WUI-330 on GitHub (new tab)",
      "https://github.com/acme/wallet-ui/pull/330",
    ],
    [mention.title, "Open mention on acme/wallet-ui#333 on GitHub (new tab)", mention.url],
  ] as const;
  for (const [title, external, url] of privateRows) {
    expect(screen.getByText(title)).toBeVisible();
    expect(screen.queryByRole("link", { name: title })).toBeNull();
    expect(screen.getByRole("link", { name: external })).toHaveAttribute("href", url);
  }
  expect(screen.getByRole("link", { name: "fix(rpc): close idle connections" })).toHaveAttribute(
    "href",
    "/pr/acme/node-client/476",
  );
});
