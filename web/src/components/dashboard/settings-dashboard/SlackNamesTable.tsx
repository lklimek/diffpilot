import { useId } from "react";
import type { SlackName } from "../../../api/generated/types";
import { buttonClass } from "../../button/button";
import { HelpTip } from "../../settings/HelpTip";
import { Tooltip } from "../../tooltip/Tooltip";
import { ERROR, FIELD } from "./look";
import { MAX_SLACK_NAMES, type FieldErrors } from "./validation";

const TH =
  "border-b border-border px-2 py-1.5 text-left text-[11px] tracking-[.4px] whitespace-nowrap text-muted uppercase";
const TD = "border-b border-border px-2 py-1.5";

interface SlackNamesTableProps {
  rows: SlackName[];
  onChange: (rows: SlackName[]) => void;
  /** Errors of the form, keyed `slack_names` and `slack_names.<index>.<login|slack>` (plan §F.6 G18). */
  errors: FieldErrors;
}

/** The Slack names table (G18): a GitHub login and the text Copy for Slack writes for it, one row each. */
export function SlackNamesTable({ rows, onChange, errors }: SlackNamesTableProps) {
  const id = useId();
  const set = (index: number, change: Partial<SlackName>) =>
    onChange(rows.map((row, i) => (i === index ? { ...row, ...change } : row)));
  const full = rows.length >= MAX_SLACK_NAMES;
  return (
    <div className="grid content-start gap-2">
      <p className="text-xs text-muted">
        Copy for Slack in Waiting on others writes these names instead of @login; other logins stay @login.
      </p>
      {errors.slack_names && <p className={ERROR}>{errors.slack_names}</p>}
      {rows.length === 0 ? (
        <p className="flex items-center gap-1.5 text-xs text-muted">
          None. Copy for Slack writes @login for every login.
          <HelpTip topic="slack_names" />
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <caption className="sr-only">Slack names</caption>
            <thead>
              <tr>
                <th scope="col" className={TH}>
                  GitHub login
                </th>
                <th scope="col" className={TH}>
                  <span className="inline-flex items-center gap-1.5">
                    Slack name
                    <HelpTip topic="slack_names" />
                  </span>
                </th>
                <th scope="col" className="border-b border-border">
                  <span className="sr-only">Remove</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row, i) => {
                const name = row.login.trim() || `row ${i + 1}`;
                const error = (field: "login" | "slack") => errors[`slack_names.${i}.${field}`];
                const errorId = (field: "login" | "slack") => `${id}-${i}-${field}`;
                const message = (field: "login" | "slack") =>
                  error(field) && (
                    <p id={errorId(field)} className={ERROR}>
                      {error(field)}
                    </p>
                  );
                return (
                  <tr key={i} className="align-top">
                    <td className={TD}>
                      <input
                        aria-label={`GitHub login ${i + 1}`}
                        value={row.login}
                        placeholder="github-login"
                        autoComplete="off"
                        aria-invalid={error("login") ? true : undefined}
                        aria-describedby={error("login") ? errorId("login") : undefined}
                        className={`${FIELD} w-40 font-mono`}
                        onChange={(e) => set(i, { login: e.target.value })}
                      />
                      {message("login")}
                    </td>
                    <td className={TD}>
                      <input
                        aria-label={`Slack name of ${name}`}
                        value={row.slack}
                        placeholder="@name or <@U01ABC>"
                        autoComplete="off"
                        aria-invalid={error("slack") ? true : undefined}
                        aria-describedby={error("slack") ? errorId("slack") : undefined}
                        className={`${FIELD} w-full min-w-44`}
                        onChange={(e) => set(i, { slack: e.target.value })}
                      />
                      {message("slack")}
                    </td>
                    <td className={TD}>
                      <Tooltip tip={`Removes the Slack name of ${name}. Save to apply.`}>
                        <button
                          type="button"
                          aria-label={`Remove the Slack name of ${name}`}
                          className={buttonClass("plain", true)}
                          onClick={() => onChange(rows.filter((_, j) => j !== i))}
                        >
                          Remove
                        </button>
                      </Tooltip>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      <Tooltip
        tip={
          full
            ? `At most ${MAX_SLACK_NAMES} Slack names. Remove one to add another.`
            : "Adds an empty row for a GitHub login and its Slack name. Save to apply."
        }
      >
        <button
          type="button"
          aria-disabled={full || undefined}
          className={`${buttonClass("plain", true)} w-fit`}
          onClick={() => !full && onChange([...rows, { login: "", slack: "" }])}
        >
          Add a Slack name
        </button>
      </Tooltip>
    </div>
  );
}
