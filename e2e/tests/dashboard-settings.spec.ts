// Settings of My work and who reads it (ADR-019, threat model T16, T20): GitHub sync and Repositories save and
// validate their changes, LLM & usage saves the ranking effort; a personal access token
// reads with only its bearer, cannot write, stops at revoke; a second user sees nothing of the first.
import { execFileSync } from "node:child_process";
import { checkScreen, signIn } from "../support/app";
import {
  allowLogin,
  api,
  JUDGE_DATA,
  latestRefresh,
  nextGithubUser,
  ranked,
  refreshButton,
  saveSettings,
  seedTodos,
  waitForRefresh,
  worldSettings,
} from "../support/dashboard";
import { expect, test } from "../support/fixtures";
import { screens } from "../support/screens";

test.describe.configure({ mode: "serial" });

const TOKEN = /^srp_[A-Za-z0-9]{8}_[A-Za-z0-9_-]{43}$/;

test("Dashboard settings: client and server validation, save, reload; the ranking effort", async ({
  page,
}) => {
  await signIn(page);
  await page.goto("/settings/github-sync");
  const scope = page.getByRole("region", { name: "Scope" });
  await expect(scope).toContainText("Your login alice is always scanned.");

  // Scan scope: linked logins are logins, never the own one.
  const login = scope.getByLabel("Add a linked login");
  const addLogin = scope.getByRole("button", { name: "Add to linked logins" });
  await login.fill("alice");
  await addLogin.click();
  await expect(scope).toContainText('Linked logins: "alice" is your own login, which is always scanned.');
  await login.fill("not a login!");
  await addLogin.click();
  await expect(scope).toContainText('Linked logins: "not a login!" is not a GitHub login');
  await login.fill("alice-helper");
  await addLogin.click();
  await expect(scope.getByRole("button", { name: "Remove alice-helper" })).toBeVisible();

  // The nightly scan: a time in 15-minute steps and an IANA zone.
  const schedule = page.getByRole("region", { name: "Nightly scan" });
  await schedule.getByLabel("Time of day").selectOption("07:15");
  await schedule.getByLabel("Time zone").selectOption("Europe/Warsaw");
  const saveDashboardSettings = () =>
    page
      .getByRole("button", { name: "Save", exact: true })
      .and(page.getByRole("button", { description: "Saves the changes on this page." }));
  await saveDashboardSettings().click();
  await expect(page.getByText("Dashboard settings saved.", { exact: true })).toBeVisible();
  await checkScreen(page);
  await screens(page, "settings-dashboard");

  await page.reload();
  await expect(schedule.getByLabel("Time of day")).toHaveValue("07:15");

  await page.getByRole("link", { name: "MemCan", exact: true }).click();
  // MemCan projects: extra names are validated here.
  const sources = page.getByRole("region", { name: "TODO sources" });
  const project = sources.getByLabel("Add a MemCan project");
  const addProject = sources.getByRole("button", { name: "Add to extra memcan projects" });
  await project.fill("release notes!");
  await addProject.click();
  await expect(sources).toContainText('Extra MemCan projects: "release notes!" is not a project name');
  await project.fill("release-notes");
  await addProject.click();
  await expect(sources.getByRole("button", { name: "Remove release-notes" })).toBeVisible();
  await saveDashboardSettings().click();
  await expect(page.getByText("Dashboard settings saved.", { exact: true })).toBeVisible();

  // Repositories: owner/name is validated on Save. Saving here preserves the GitHub sync fields.
  await page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Repositories" }).click();
  await expect(page).toHaveURL(/\/settings\/repositories$/);
  const repositories = page.getByRole("region", { name: "Repository rules" });
  await repositories.getByRole("button", { name: "Add repository" }).click();
  await repositories.getByLabel("Repository 1").fill("acme platform");
  await saveDashboardSettings().click();
  const validation = page.getByRole("alert").filter({ hasText: "Not saved: fix the marked fields." });
  await expect(validation).toContainText("Repository 1: use owner/name.");
  await checkScreen(page);
  await repositories.getByLabel("Repository 1").fill("acme/platform");
  await repositories.getByLabel("Short name of acme/platform").fill("PLAT");
  await saveDashboardSettings().click();
  await expect(page.getByText("Dashboard settings saved.", { exact: true })).toBeVisible();
  await page.reload();
  await expect(repositories.getByLabel("Short name of acme/platform")).toHaveValue("PLAT");

  const saved = (await (await page.request.get("/api/v1/settings/dashboard")).json()) as {
    settings: ReturnType<typeof worldSettings>;
    memcan_projects: string[];
  };
  expect(saved.settings).toMatchObject({
    linked_logins: ["alice-helper"],
    repositories: [{ repo: { owner: "acme", name: "platform" }, alias: "PLAT" }],
    memcan_extra_projects: ["release-notes"],
    schedule_enabled: true,
    schedule_time: "07:15",
    time_zone: "Europe/Warsaw",
  });
  expect(saved.memcan_projects).toEqual(["platform", "release-notes"]);

  // The server checks every rule too and names the field.
  const offStep = await api(page, "PUT", "/api/v1/settings/dashboard", {
    ...saved.settings,
    schedule_time: "07:10",
  });
  expect(offStep.status).toBe(422);
  expect(offStep.body?.code).toBe("invalid_dashboard_settings");
  expect(String(offStep.body?.message)).toMatch(/schedule_time|time of day/i);
  const zone = await api(page, "PUT", "/api/v1/settings/dashboard/time-zone", { time_zone: "Mars/Olympus" });
  expect([zone.status, zone.body?.code]).toEqual([422, "invalid_dashboard_settings"]);

  // The ranking effort is installation-wide, saved by the section's Save.
  await page.goto("/settings/llm");
  const selection = page.getByRole("region", { name: "Model and effort" });
  const effort = selection.getByLabel("Dashboard ranking effort");
  await expect(effort).toHaveValue("low");
  await effort.selectOption("medium");
  await selection.getByRole("button", { name: "Save", exact: true }).click();
  await expect(selection.getByRole("status")).toHaveText("Saved");
  await page.reload();
  await expect(page.getByLabel("Dashboard ranking effort")).toHaveValue("medium");
  await checkScreen(page);
});

test("a personal access token reads with its bearer only, cannot write, and stops at revoke", async ({
  page,
  stack,
}) => {
  await signIn(page);
  await page.goto("/settings");
  const card = page.getByRole("region", { name: "Access tokens" });
  await expect(card).toContainText("No tokens yet.");
  await card.getByRole("button", { name: "New token" }).click();
  const dialog = page.getByRole("dialog", { name: "New access token" });
  await dialog.getByLabel("Name").fill("e2e agent");
  await dialog.getByRole("button", { name: "Create token" }).click();
  const created = page.getByRole("dialog", { name: "Token created" });
  const secret = await created.getByLabel("Your new token").inputValue();
  expect(secret).toMatch(TOKEN);
  stack.secrets.push(["personal access token", secret]);
  await expect(created).toContainText("Store it now; it cannot be shown again.");
  await checkScreen(page);
  await created.getByRole("button", { name: "Done" }).click();
  await expect(card.getByRole("row", { name: /e2e agent/ })).toContainText(`${secret.slice(0, 12)}…`);
  await expect(card).not.toContainText(secret);

  // Only the bearer: no cookie, no CSRF token.
  const bearer = { authorization: `Bearer ${secret}` };
  const read = (path: string, init: RequestInit = {}) =>
    fetch(`${stack.url}${path}`, { ...init, headers: { ...bearer, ...init.headers } });
  const me = await read("/api/v1/me");
  expect(me.status).toBe(200);
  expect(await me.json()).toMatchObject({ user: { login: "alice" }, csrf_token: null });
  expect((await read("/api/v1/dashboard")).status).toBe(200);
  expect((await read("/api/v1/dashboard/items")).status).toBe(200);
  expect((await read("/api/v1/settings/dashboard")).status).toBe(200);
  const curl = execFileSync(
    "curl",
    [
      "-s",
      "-o",
      "/dev/null",
      "-w",
      "%{http_code}",
      "-H",
      `Authorization: Bearer ${secret}`,
      `${stack.url}/api/v1/dashboard`,
    ],
    { encoding: "utf8" },
  );
  expect(curl).toBe("200");
  for (const [method, path] of [
    ["POST", "/api/v1/dashboard/refreshes"],
    ["PUT", "/api/v1/settings/dashboard"],
    ["GET", "/api/v1/settings/access-tokens"],
  ] as const) {
    const refused = await read(path, {
      method,
      headers: { "content-type": "application/json" },
      body: method === "GET" ? undefined : "{}",
    });
    expect([path, refused.status, ((await refused.json()) as { code: string }).code]).toEqual([
      path,
      403,
      "token_not_accepted",
    ]);
  }

  await card.getByRole("button", { name: "Revoke e2e agent" }).click();
  await card
    .getByRole("group", { name: "Confirm revoking e2e agent" })
    .getByRole("button", { name: "Revoke token" })
    .click();
  await expect(card).toContainText("No tokens yet.");
  const revoked = await read("/api/v1/dashboard");
  expect(revoked.status).toBe(401);
  expect(revoked.headers.get("www-authenticate")).toMatch(/^Bearer/);
  await checkScreen(page);
});

test("a second user sees none of the first user's dashboard, refreshes, events or tokens", async ({
  page,
  browser,
  stack,
}) => {
  test.skip(
    !stack.stateDir || !stack.github || !stack.memcan,
    "needs a local stack (allow-list, fake GitHub)",
  );
  stack.secrets.push(...JUDGE_DATA);
  await signIn(page);
  await saveSettings(page, worldSettings());
  await seedTodos(page, stack);
  const previous = (await latestRefresh(page))?.id;
  await refreshButton(page).click();
  const refresh = await waitForRefresh(page, (current) => current.id !== previous && ranked(current));
  const created = await api(page, "POST", "/api/v1/settings/access-tokens", {
    name: "alice agent",
    scopes: ["read"],
    expires_in_days: 30,
  });
  expect(created.status).toBe(201);
  const token = String(created.body?.token);
  const tokenId = String((created.body?.access_token as { id: string }).id);
  stack.secrets.push(["personal access token of alice", token]);

  allowLogin(stack, "bob");
  await nextGithubUser(page, stack, "bob", 9_000_002);
  const context = await browser.newContext({ baseURL: stack.url });
  try {
    const bob = await context.newPage();
    await signIn(bob, "bob");
    await expect(bob.getByText("Nothing scanned yet.")).toBeVisible();
    await checkScreen(bob);
    expect(await latestRefresh(bob)).toBeNull();
    const summary = (await (await bob.request.get("/api/v1/dashboard")).json()) as { snapshot: unknown };
    expect(summary.snapshot).toBeNull();
    const items = (await (await bob.request.get("/api/v1/dashboard/items")).json()) as { items: unknown[] };
    expect(items.items).toEqual([]);
    for (const path of [
      `/api/v1/dashboard/refreshes/${refresh.id}`,
      `/api/v1/dashboard/refreshes/${refresh.id}/events`,
    ]) {
      expect([path, (await bob.request.get(path)).status()]).toEqual([path, 404]);
    }
    expect((await api(bob, "POST", `/api/v1/dashboard/refreshes/${refresh.id}/rank`)).status).toBe(404);
    const settings = (await (await bob.request.get("/api/v1/settings/dashboard")).json()) as {
      own_login: string;
      settings: { repositories: unknown[] };
    };
    expect([settings.own_login, settings.settings.repositories]).toEqual(["bob", []]);
    expect(await (await bob.request.get("/api/v1/settings/access-tokens")).json()).toEqual([]);
    expect((await api(bob, "DELETE", `/api/v1/settings/access-tokens/${tokenId}`)).status).toBe(404);
  } finally {
    await context.close();
  }
  // Alice's token still reads her own dashboard, and her refresh's events (one stream, closed at once).
  const events = new AbortController();
  const stream = await fetch(`${stack.url}/api/v1/dashboard/refreshes/${refresh.id}/events`, {
    headers: { authorization: `Bearer ${token}` },
    signal: events.signal,
  });
  try {
    expect(stream.status).toBe(200);
    expect(stream.headers.get("content-type")).toMatch(/^text\/event-stream/);
    const reader = stream.body!.getReader();
    let received = "";
    while (!received.includes(refresh.id)) {
      const { value, done } = await reader.read();
      if (done) break;
      received += new TextDecoder().decode(value);
    }
    expect(received).toContain(refresh.id);
  } finally {
    events.abort();
  }
  const mine = await fetch(`${stack.url}/api/v1/dashboard`, {
    headers: { authorization: `Bearer ${token}` },
  });
  expect(mine.status).toBe(200);
  expect(((await mine.json()) as { latest_refresh: { id: string } }).latest_refresh.id).toBe(refresh.id);
});
