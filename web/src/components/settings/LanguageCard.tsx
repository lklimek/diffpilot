import { useId } from "react";
import type { LlmLanguage } from "../../api/generated/types";
import { SettingsCard } from "./Card";
import { HelpTip } from "./HelpTip";

const LANGUAGES: Record<LlmLanguage, string> = { en: "English", pl: "Polski", ru: "Русский" };

interface LanguageCardProps {
  /** Current language of LLM texts. */
  language: LlmLanguage;
  /** Saving: the choice is disabled. */
  busy: boolean;
  /** Called with the chosen language. */
  onChange: (language: LlmLanguage) => void;
}

/** Language of LLM-written texts. */
export function LanguageCard({ language, busy, onChange }: LanguageCardProps) {
  const id = useId();
  return (
    <SettingsCard title="Language of LLM texts" help={<HelpTip topic="language" />}>
      <label htmlFor={id}>Explanations, questions and assessments are written in</label>
      <select
        id={id}
        value={language}
        disabled={busy}
        onChange={(event) => onChange(event.target.value as LlmLanguage)}
        className="w-fit rounded-md border border-muted bg-surface px-2 py-1"
      >
        {Object.entries(LANGUAGES).map(([value, name]) => (
          <option key={value} value={value}>
            {name}
          </option>
        ))}
      </select>
      <p className="text-xs text-muted">
        The interface stays English. Analyses started after a change use the new language. Comments you send
        to GitHub are what you write.
      </p>
    </SettingsCard>
  );
}
