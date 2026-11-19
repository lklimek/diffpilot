import { useId, useState, type ReactNode } from "react";
import { buttonClass } from "../../button/button";
import { Tooltip } from "../../tooltip/Tooltip";
import { ERROR, FIELD } from "./look";

interface ListEditorProps {
  /** Name of the list. */
  label: string;
  /** Accessible name of the entry field, e.g. "Add a linked login". */
  addLabel: string;
  items: string[];
  /** Said when the list is empty. */
  empty: string;
  /** Why a value cannot be added (syntax, duplicate, limit), or null. */
  check: (value: string, items: string[]) => string | null;
  onChange: (items: string[]) => void;
  /** The list breaks a rule (shown after a save attempt). */
  error?: string;
  placeholder?: string;
  /** Extra mark after an entry, e.g. "not visible to your GitHub token". */
  mark?: (item: string) => ReactNode;
  /** The `?` help next to the name. */
  help?: ReactNode;
  /** A short visible hint under the name, also describing the entry field. */
  hint?: string;
}

/** A short list of names edited one at a time (logins, owners, bot authors, project names). */
export function ListEditor({
  label,
  addLabel,
  items,
  empty,
  check,
  onChange,
  error,
  placeholder,
  mark,
  help,
  hint,
}: ListEditorProps) {
  const id = useId();
  const [text, setText] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const message = problem ?? error;
  const add = () => {
    const value = text.trim();
    if (value === "") return;
    const refused = check(value, items);
    setProblem(refused);
    if (refused) return;
    onChange([...items, value]);
    setText("");
  };
  return (
    <div className="grid content-start gap-1.5">
      <span className="flex items-center gap-2">
        <span id={`${id}-label`} className="text-sm font-semibold">
          {label}
        </span>
        {help}
      </span>
      {hint && (
        <p id={`${id}-hint`} className="m-0 text-xs text-muted">
          {hint}
        </p>
      )}
      {items.length > 0 ? (
        <ul aria-labelledby={`${id}-label`} className="m-0 flex list-none flex-wrap gap-1.5 p-0">
          {items.map((item) => (
            <li
              key={item}
              className="inline-flex items-center gap-1 rounded border border-border bg-surface-2 py-0.5 pr-0.5 pl-1.5 font-mono text-[12.5px]"
            >
              {item}
              {mark?.(item)}
              <Tooltip tip={`Removes ${item} from the list. Save to apply.`}>
                <button
                  type="button"
                  aria-label={`Remove ${item}`}
                  className="inline-flex min-h-6 min-w-6 cursor-pointer items-center justify-center rounded text-muted hover:text-text"
                  onClick={() => onChange(items.filter((i) => i !== item))}
                >
                  ×
                </button>
              </Tooltip>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-xs text-muted">{empty}</p>
      )}
      <div className="flex gap-2">
        <input
          aria-label={addLabel}
          value={text}
          placeholder={placeholder}
          autoComplete="off"
          aria-invalid={message ? true : undefined}
          aria-describedby={
            [hint && `${id}-hint`, message && `${id}-error`].filter(Boolean).join(" ") || undefined
          }
          className={`${FIELD} w-56`}
          onChange={(e) => {
            setText(e.target.value);
            setProblem(null);
          }}
          onKeyDown={(e) => {
            if (e.key !== "Enter") return;
            e.preventDefault();
            add();
          }}
        />
        <Tooltip tip={`Adds the typed name to ${label.toLowerCase()}. Save to apply.`}>
          <button
            type="button"
            aria-label={`Add to ${label.toLowerCase()}`}
            className={buttonClass("plain", true)}
            onClick={add}
          >
            Add
          </button>
        </Tooltip>
      </div>
      {message && (
        <p id={`${id}-error`} className={ERROR}>
          {message}
        </p>
      )}
    </div>
  );
}
