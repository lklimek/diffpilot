import { useId } from "react";
import type { LlmLanguage } from "../../api/generated/types";
import { SettingsCard } from "./Card";
import { FormRow } from "./FormRow";
import { HelpTip } from "./HelpTip";

import { LANGUAGE_OPTION } from "../language-tag/LanguageTag";

interface LanguageCardProps {
  /** Current language of LLM texts. */
  language: LlmLanguage;
  /** Saving: the choice is disabled. */
  busy: boolean;
  error?: string;
  /** Called with the chosen language. */
  onChange: (language: LlmLanguage) => void;
}

/** Language of LLM-written texts. */
export function LanguageCard({ language, busy, error, onChange }: LanguageCardProps) {
  const id = useId();
  return (
    <SettingsCard title="Language of LLM texts" help={<HelpTip topic="language" />}>
      <FormRow
        label="Explanations, questions and assessments are written in"
        htmlFor={id}
        help="Analyses started after a change use the new language. The interface stays English."
      >
        <select
          id={id}
          value={language}
          disabled={busy}
          onChange={(event) => onChange(event.target.value as LlmLanguage)}
          className="w-fit rounded-md border border-muted bg-surface px-2 py-1"
        >
          {Object.entries(LANGUAGE_OPTION).map(([value, name]) => (
            <option key={value} value={value}>
              {name}
            </option>
          ))}
        </select>
      </FormRow>
      {error && <p role="alert">The language was not saved: {error}</p>}
    </SettingsCard>
  );
}
