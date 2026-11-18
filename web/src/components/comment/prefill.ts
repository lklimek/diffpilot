// Draft review comments made from LLM text ("Use as review comment"): plain text, marked as LLM-suggested.
import type { Annotation } from "../../api/generated/types";

/** First line of every prefilled draft; the user may delete it before posting. */
export const LLM_SUGGESTED = "LLM-suggested — review before posting";

const suggested = (body: string) => `${LLM_SUGGESTED}\n\n${body.trim()}`;

/** A flag as a draft comment: its title, the scenario as a numbered list, what was not checked. */
export function flagComment(flag: Annotation): string {
  const scenario = (flag.flag?.scenario ?? []).map((line, i) => `${i + 1}. ${line}`);
  const notChecked = flag.flag?.not_checked.trim();
  const parts = [[flag.text?.trim() ?? "", ...scenario].join("\n")];
  if (notChecked) parts.push(`Not checked: ${notChecked}`);
  return suggested(parts.join("\n\n"));
}

/** An Ask answer as a draft comment, as written (line breaks and lists kept). */
export const answerComment = (answer: string): string => suggested(answer);

/** `text` after an existing draft and a blank line; an empty draft is replaced. */
export function appendDraft(draft: string, text: string): string {
  const kept = draft.trimEnd();
  return kept ? `${kept}\n\n${text}` : text;
}
