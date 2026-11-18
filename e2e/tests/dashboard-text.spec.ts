// Threat model T17 / T12 on My work (`dashboard_llm_text_renders_as_text`): hostile MemCan TODO text and hostile LLM
// judgement text (why, next step, alarm) with markup, markdown links and `javascript:` URLs render as text: no
// element is created from them, nothing runs, nothing navigates.
import { signIn } from "../support/app";
import {
  JUDGE_DATA,
  latestRefresh,
  ranked,
  refreshButton,
  saveSettings,
  seedTodos,
  waitForRefresh,
  workList,
  worldSettings,
} from "../support/dashboard";
import { expect, test } from "../support/fixtures";

test.describe.configure({ mode: "serial" });

const MARK = "e2e-hostile";
const TITLE = `<img src=x onerror="window.__xss=1"> ${MARK} [open](javascript:window.__xss=2)`;
const DESCRIPTION =
  '<script>window.__xss=3</script> see javascript:window.__xss=4 and <a href="javascript:x">here</a>';
const WHY = '<img src=x onerror="window.__xss=5"> [the docs](javascript:window.__xss=6)';
const NEXT = '<a href="javascript:window.__xss=7">do it</a> **now**';
const ALARM = "<script>window.__xss=8</script> [alarm](javascript:alert(1))";

/** Elements only hostile text could have created; the page has none of its own. */
const INJECTED = '#root [onerror], #root script, #root a[href^="javascript:" i], #root img[src="x"]';

interface Part {
  component: string;
  source: string;
}
interface Item {
  id: string;
  key: string;
  kind: string;
  score: { parts: Part[] };
}

test("dashboard_llm_text_renders_as_text: hostile TODO and LLM text stays text", async ({ page, stack }) => {
  test.skip(
    !stack.script || !stack.memcan,
    "needs the fake provider's script and fake MemCan (a local stack)",
  );
  stack.secrets.push(...JUDGE_DATA);
  await signIn(page);
  await saveSettings(page, worldSettings());
  await seedTodos(page, stack);
  await refreshButton(page).click();
  await waitForRefresh(page, ranked);

  // The TODOs change (judged again, alone in one batch); the judge answers them with hostile text, asked parts 0.
  const listed = (await (await page.request.get("/api/v1/dashboard/items?limit=500")).json()) as {
    items: Item[];
  };
  const todos = listed.items.filter((item) => item.kind === "todo");
  expect(todos).toHaveLength(2);
  const answer = (item: Item, alarm: string | null) => {
    const asked = item.score.parts.filter((part) => part.source === "llm").map((part) => part.component);
    return {
      key: item.key,
      defect: asked.includes("defect_severity") ? { likelihood: 0, impact: 0, basis: WHY } : null,
      parts: asked
        .filter((component) => component !== "defect_severity")
        .map((component) => ({ component, points: 0, basis: NEXT })),
      eligibility: null,
      also_keys: [],
      alarm,
      why: WHY,
      next: NEXT,
    };
  };
  stack.script?.([
    {
      id: "hostile-judge",
      job: "judge_work",
      prompt_contains: MARK,
      action: { answer: { output: { items: [answer(todos[0]!, ALARM), answer(todos[1]!, null)] } } },
    },
  ]);
  await seedTodos(page, stack, (todo) => ({
    ...todo,
    title: `${TITLE} ${todo.id}`,
    description: DESCRIPTION,
  }));
  const shown = await latestRefresh(page);
  await refreshButton(page).click();
  const done = await waitForRefresh(page, (r) => r.id !== shown?.id && ranked(r));
  expect(done.judged, "both TODOs judged with the hostile answer").toBe(2);

  // Every hostile text is on the page as typed: the alarm row, the titles, and each row's details.
  await expect(page.getByText(ALARM, { exact: false }).first()).toBeVisible();
  for (const item of todos) {
    const details = workList(page).getByRole("button", { name: `Details of ${item.id}` });
    if ((await details.count()) > 0 && (await details.getAttribute("aria-expanded")) === "false")
      await details.click();
  }
  const text = await page.locator("#root").innerText();
  for (const hostile of [TITLE, DESCRIPTION, WHY, NEXT]) expect(text).toContain(hostile);
  await expect(page.locator(INJECTED)).toHaveCount(0);
  expect(await page.evaluate(() => (window as { __xss?: number }).__xss)).toBeUndefined();
  await expect(page).toHaveURL(/\/$/);
});
