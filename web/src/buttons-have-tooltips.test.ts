// @vitest-environment node
import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

// Brief §4 rule 3 / plan §12.4: every button has a tooltip explaining it. A button outside a `<Tooltip>` must
// be on this list, with the reason it does without one.
const ALLOWED: { file: string; match: string; reason: string }[] = [
  {
    file: "components/code-view/CodeView.tsx",
    match: "data-line-action",
    reason:
      "Line numbers: one per displayed line (hundreds); each has an aria-label and a native title naming the action.",
  },
  {
    file: "components/code-view/markers.tsx",
    match: 'data-mark="remark"',
    reason:
      "LLM remark marker: hover and focus show its popover with the remark text, which serves as its tooltip.",
  },
  {
    file: "components/code-view/markers.tsx",
    match: 'data-mark="thread"',
    reason:
      "GitHub thread marker: hover and focus show its popover with the thread excerpt, which serves as its tooltip.",
  },
  {
    file: "components/ask/AskForm.tsx",
    match: "aria-expanded",
    reason:
      "Ask's section disclosure header, like the other Inspector sections: named by its caption, state announced.",
  },
  {
    file: "components/inspector/Collapsible.tsx",
    match: "aria-expanded",
    reason: "Section disclosure header: named by its section title, its expanded state is announced.",
  },
  {
    file: "components/nav/NavList.tsx",
    match: 'aria-label="PR context',
    reason: "Navigation row named by its destination; a tip would cover the rows below.",
  },
  {
    file: "components/nav/NavList.tsx",
    match: 'current?.block === "flow_intro"',
    reason: "Flow row named by its destination; its risk and group labels carry their own tooltips.",
  },
  {
    file: "components/nav/NavList.tsx",
    match: 'current?.block === "step"',
    reason: "Step row named by its destination; a tip would cover the rows below.",
  },
  {
    file: "screens/tour/DiagramPane.tsx",
    match: "aria-expanded",
    reason: "Disclosure header (Show/Hide flow diagram): its text says what it does, its state is announced.",
  },
  {
    file: "components/help/HelpDialog.tsx",
    match: "onClick={onClose}",
    reason: "Close of the modal help dialog: takes the focus on open, where a tip would cover the dialog.",
  },
  {
    file: "screens/tour/DiagramPane.tsx",
    match: "onSelectStep(node.step_id)",
    reason: "Text alternative of the diagram: link-style list of steps, each named by its step title.",
  },
];

const root = new URL(".", import.meta.url).pathname;

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sources(path);
    return entry.name.endsWith(".tsx") && !entry.name.includes(".test.") ? [path] : [];
  });
}

/** Buttons outside every `<Tooltip>…</Tooltip>` of `code`: the source from `<button` to its `</button>`. */
export function untipped(code: string): string[] {
  const tips = [...code.matchAll(/<Tooltip\b[\s\S]*?<\/Tooltip>/g)].map((m) => [
    m.index,
    m.index + m[0].length,
  ]);
  return [...code.matchAll(/<(button|Button)\b[\s\S]*?<\/\1>/g)]
    .filter((m) => !tips.some(([from, to]) => from! <= m.index && m.index < to!))
    .map((m) => m[0]);
}

describe("buttons", () => {
  const files = [join(root, "components"), join(root, "screens")].flatMap(sources);
  const found = files.flatMap((file) =>
    untipped(readFileSync(file, "utf8")).map((button) => ({ file: relative(root, file), button })),
  );

  it("each has a tooltip, or is on the allow-list with a reason", () => {
    const missing = found
      .filter(({ file, button }) => !ALLOWED.some((a) => a.file === file && button.includes(a.match)))
      .map(({ file, button }) => `${file}: ${button.replace(/\s+/g, " ").slice(0, 120)}`);
    expect(missing).toEqual([]);
  });

  it("the allow-list has no stale entry", () => {
    const unused = ALLOWED.filter(
      (a) => !found.some(({ file, button }) => a.file === file && button.includes(a.match)),
    );
    expect(unused).toEqual([]);
  });

  it("the check finds a button outside a tooltip and ignores one inside", () => {
    expect(untipped('<Tooltip tip="x"><button>A</button></Tooltip><button>B</button>')).toEqual([
      "<button>B</button>",
    ]);
  });
});
