// Self-test of checkScreen (support/app.ts) on static pages, no stack: it passes a clean screen and fails on each
// kind of violation it exists to catch.
import { expect, test } from "@playwright/test";
import { checkScreen } from "../support/app";

// A tooltip as web/src/components/tooltip/Tooltip.tsx renders it: shown on focus, hidden on blur.
const tooltip = (trigger: string, tip: string) => `
  <span class="tip"><button type="button" aria-describedby="t-${trigger}">${trigger}</button>
  <span id="t-${trigger}" role="tooltip" hidden>${tip}</span></span>`;
const page = (
  top: string,
  main: string,
) => `<!doctype html><html lang="en"><head><title>check</title></head><body>
  <header><span>acme/node-client #482</span>${top}</header><main><h1>Review</h1>${main}</main>
  <script>
    for (const wrapper of document.querySelectorAll(".tip")) {
      const tip = wrapper.querySelector("[role=tooltip]");
      wrapper.addEventListener("focusin", () => { if (!wrapper.dataset.broken) tip.hidden = false; });
      wrapper.addEventListener("focusout", () => { if (!wrapper.dataset.sticky) tip.hidden = true; });
    }
  </script></body></html>`;

// The PR opener (web/src/components/pr-form/PrUrlForm.tsx): a URL field and its submit control.
const prOpener = (submit: string) =>
  `<form><label>GitHub pull request URL <input type="text" inputmode="url"></label>${submit.replace('type="button"', 'type="submit"')}</form>`;
// An unrelated URL field can also opt into the URL keyboard. The opener rule must check the field's label.
const channel = `<form novalidate><label>URL <input type="url" inputmode="url" required></label>${tooltip("Save", "Saves the channel.")}</form>`;

const cases: [string, string, RegExp | null][] = [
  ["a clean screen", page(tooltip("Ticks", "Steps you marked reviewed."), "<p>3 of 7 steps</p>"), null],
  ["a banned phrase in the page", page("", "<p>No risk here.</p>"), /banned wording on the page/],
  ["observed Codex plan usage", page("", "<p data-codex-plan-usage>Primary plan usage: 12.5%</p>"), null],
  [
    "a banned word in plan usage",
    page("", "<p data-codex-plan-usage>confidence: 12.5%</p>"),
    /banned wording on the page/,
  ],
  [
    "plan usage percentage in the top bar",
    page("<p data-codex-plan-usage>Plan usage: 12.5%</p>", ""),
    /top bar/,
  ],
  ["a percentage in the page", page("", "<p>Covered: 80 %</p>"), /banned wording on the page/],
  ["any % in the top bar", page("<span>progress 3/7 (%)</span>", ""), /top bar/],
  ["a banned word in a tooltip", page(tooltip("Fact", "Verified by the LLM."), ""), /banned wording/],
  [
    "a tooltip that does not open on focus",
    page(tooltip("Fact", "From the diff.").replace('class="tip"', 'class="tip" data-broken="1"'), ""),
    /tooltip not shown on focus/,
  ],
  [
    "a tooltip that stays open after focus leaves",
    page(tooltip("Fact", "From the diff.").replace('class="tip"', 'class="tip" data-sticky="1"'), ""),
    /stays open after focus leaves/,
  ],
  [
    "an aria-disabled trigger (focusable, its tip says why)",
    page(
      tooltip("Previous", "No earlier block.").replace('type="button"', 'type="button" aria-disabled="true"'),
      "",
    ),
    null,
  ],
  [
    "a natively disabled trigger (its tip cannot be reached by keyboard)",
    page(tooltip("Previous", "No earlier block.").replace('type="button"', 'type="button" disabled'), ""),
    /cannot take keyboard focus: .*Previous/,
  ],
  ["an empty tooltip", page(tooltip("Fact", ""), ""), /tooltip is empty: .*Fact/],
  [
    "aria-describedby naming no element",
    page("", '<button type="button" aria-describedby="gone">Fact</button>'),
    /aria-describedby names no element: .*gone/,
  ],
  [
    "PR opener with Open and its tooltip",
    page("", prOpener(tooltip("Open", "Opens the pull request."))),
    null,
  ],
  [
    "PR opener with Open but no tooltip",
    page("", prOpener('<button type="submit">Open</button>')),
    /required tooltip missing: Open pull request .*Open/,
  ],
  [
    "PR opener without Open",
    page("", prOpener(tooltip("Check", "Looks the pull request up."))),
    /required control missing: Open pull request \(0 of 1 rendered\)/,
  ],
  ["a notification channel's URL form with its own label", page("", channel), null],
  [
    "a trigger without keyboard focus",
    page(
      tooltip("Fact", "From the diff.").replace(
        /<button type="button"(.*?)>Fact<\/button>/,
        "<span$1>Fact</span>",
      ),
      "",
    ),
    /cannot take keyboard focus/,
  ],
  ["an axe violation", page("", '<img src="data:image/gif;base64,R0lGODlhAQABAAAAACw=">'), /image-alt/],
];

for (const [name, html, failure] of cases) {
  test(`checkScreen: ${name} ${failure ? "fails" : "passes"}`, async ({ page: browser }) => {
    await browser.setContent(html);
    const outcome = await checkScreen(browser).then(
      () => null,
      (error: Error) => error.message,
    );
    if (failure) expect(outcome ?? "passed").toMatch(failure);
    else expect(outcome).toBeNull();
  });
}
