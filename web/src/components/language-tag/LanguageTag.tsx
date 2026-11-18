import type { LlmLanguage } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

/** Name of an LLM language in English. */
export const LANGUAGE_NAME: Record<LlmLanguage, string> = { en: "English", pl: "Polish", ru: "Russian" };

/** Small tag with the language of an analysis's LLM texts (provenance-tag style); nothing when not known. */
export function LanguageTag({ language }: { language: LlmLanguage | undefined }) {
  if (!language) return null;
  return (
    <Tooltip tip={`LLM texts of this analysis are in ${LANGUAGE_NAME[language]}.`}>
      <span
        tabIndex={0}
        className="inline-block cursor-help rounded border border-border px-1 align-baseline text-[10.5px] leading-4 font-semibold whitespace-nowrap text-muted"
      >
        {language.toUpperCase()}
      </span>
    </Tooltip>
  );
}
