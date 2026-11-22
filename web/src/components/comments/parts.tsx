// Small shared pieces of the Comments pages: fact badges, chips, gate marks, item names.
import type { CommentItem, Gate, Validity } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

/** A fact diffpilot read (deterministic); the tip names the check (brief §1: badges only for facts). */
export function FactChip({ text, tip }: { text: string; tip: string }) {
  return (
    <Tooltip tip={tip}>
      <span
        tabIndex={0}
        data-fact
        className="inline-flex cursor-help items-center rounded-full border border-ok bg-ok-bg px-2 text-xs whitespace-nowrap text-ok"
      >
        {text}
      </span>
    </Tooltip>
  );
}

/** A plain chip with a tooltip. */
export function Chip({
  text,
  tip,
  tone = "plain",
}: {
  text: string;
  tip: string;
  tone?: "plain" | "reask" | "fail";
}) {
  const look = {
    plain: "border-border bg-surface-2 text-text",
    reask: "border-med-bd bg-med-bg font-semibold text-med",
    fail: "border-high-bd bg-high-bg font-semibold text-high",
  }[tone];
  return (
    <Tooltip tip={tip}>
      <span
        tabIndex={0}
        className={`inline-flex cursor-help items-center rounded-full border px-2 text-xs whitespace-nowrap ${look}`}
      >
        {text}
      </span>
    </Tooltip>
  );
}

/** Rule text of a gate item (plan §2.3). */
export const GATE_RULE = "Gate items are never grouped or decided in bulk; each needs your own decision.";

/** A gate mark: colour, text and its rule in the tip. */
export function GateChip({ gate, source }: { gate: Gate; source?: "assessment" | "owner" }) {
  const from =
    source === "owner" ? " · marked by you" : source === "assessment" ? " · from the LLM assessment" : "";
  return (
    <Tooltip tip={`Gate ${gate}${from}. ${GATE_RULE}`}>
      <span
        tabIndex={0}
        data-gate={gate}
        className="inline-flex cursor-help items-center rounded-full border border-high-bd bg-high-bg px-2 text-xs font-semibold whitespace-nowrap text-high"
      >
        {source ? `Gate: ${gate}${from}` : gate}
      </span>
    </Tooltip>
  );
}

/** `C14`, or `P24` for a PR-level question. */
export const itemId = (item: Pick<CommentItem, "no" | "kind">) =>
  `${item.kind === "pr_question" ? "P" : "C"}${item.no}`;

const SOURCE: Record<CommentItem["kind"], string> = {
  thread: "inline thread",
  review_body: "review body",
  pr_comment: "PR comment",
  pr_question: "PR-level question",
};

/** "thepastaclaw · bot · inline thread". */
export function sourceText(item: Pick<CommentItem, "author" | "author_kind" | "kind">): string {
  const kind = item.author_kind === "bot" ? "bot" : item.author_kind === "human" ? "human" : "diffpilot";
  return `${item.author} · ${kind} · ${SOURCE[item.kind] ?? item.kind}`;
}

/** "The LLM says: …" (plan §3.7). */
export const VALIDITY: Record<Validity, string> = {
  valid: "valid",
  partly_valid: "partly valid",
  invalid: "invalid",
};

/** Plain text with only `https://github.com/…` links (threat model T24); any other URL stays text. */
export function GithubLinkedText({ text }: { text: string }) {
  const parts = text.split(/(https:\/\/github\.com\/[^\s)<>"']+)/g);
  return (
    <>
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <a key={i} href={part} target="_blank" rel="noopener noreferrer">
            {part}
          </a>
        ) : (
          part
        ),
      )}
    </>
  );
}
