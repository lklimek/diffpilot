import { useEffect, useState } from "react";
import type { ControlledOwners } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { ListEditor } from "../dashboard/settings-dashboard/ListEditor";
import { dateTimeText } from "../format/time";
import { Tooltip } from "../tooltip/Tooltip";
import { SettingsCard } from "./Card";
import { HelpTip } from "./HelpTip";

const LABEL = "Controlled organisations";
/** `SetControlledOwnersRequest::MAX_OWNERS`. */
const MAX_OWNERS = 50;
// GitHub login: letters, digits and inner -, up to 39 characters (the server's rule).
const OWNER = /^[A-Za-z0-9][A-Za-z0-9-]{0,38}$/;

const asSet = (list: readonly string[]) =>
  list
    .map((o) => o.toLowerCase())
    .sort()
    .join("\n");

/** Why `value` cannot join `list`, or null. */
function check(value: string, list: string[]): string | null {
  if (!OWNER.test(value))
    return `${LABEL}: "${value}" is not a GitHub login (letters, digits and -, up to 39).`;
  if (list.some((o) => o.toLowerCase() === value.toLowerCase()))
    return `${LABEL}: "${value}" is already listed.`;
  if (list.length >= MAX_OWNERS) return `${LABEL}: at most ${MAX_OWNERS}.`;
  return null;
}

/** The installation-wide controlled GitHub owners (Dependencies view, ADR-021); the PR's owner is implicit. */
export function ControlledOwnersCard({
  owners,
  busy,
  saved,
  error,
  onSave,
  onDirtyChange,
}: {
  owners: ControlledOwners;
  busy: boolean;
  saved?: boolean;
  error?: string;
  onSave: (owners: string[]) => void;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const [draft, setDraft] = useState(owners.owners);
  const [edited, setEdited] = useState(false);
  // Each save (a new time, or a new list) replaces the draft; the card stays mounted, so focus stays.
  const savedKey = `${owners.changed_at ?? ""}\n${owners.owners.join("\n")}`;
  const [base, setBase] = useState(savedKey);
  if (base !== savedKey) {
    setBase(savedKey);
    setDraft(owners.owners);
    setEdited(false);
  }
  // Logins are case-insensitive and the server sorts the list: the same set is no change.
  const unchanged = asSet(draft) === asSet(owners.owners);
  useEffect(() => onDirtyChange?.(!unchanged), [unchanged, onDirtyChange]);
  useEffect(() => () => onDirtyChange?.(false), [onDirtyChange]);
  const off = busy || unchanged;
  return (
    <SettingsCard title="Controlled owners" help={<HelpTip topic="controlled_owners" />}>
      <ListEditor
        label={LABEL}
        addLabel="Add a controlled organisation"
        items={draft}
        empty="None listed: only each pull request's own owner is yours."
        check={check}
        onChange={(list) => {
          setDraft(list);
          setEdited(true);
        }}
        placeholder="GitHub organisation or user"
        hint="A pull request's own owner is always yours; it need not be listed."
      />
      <Tooltip
        tip={
          busy
            ? "Saving the list. Wait for it to finish."
            : unchanged
              ? "Saves the list for everyone using this installation. Nothing to save yet: the list is as saved."
              : "Saves the list for everyone using this installation; the Dependencies view uses it at once."
        }
      >
        <button
          type="button"
          aria-disabled={off || undefined}
          className={`${buttonClass("primary")} w-fit justify-self-end`}
          onClick={() => !off && onSave(draft)}
        >
          Save organisations
        </button>
      </Tooltip>
      {owners.changed_by && owners.changed_at && (
        <p className="text-xs text-muted">
          changed by {owners.changed_by} at {dateTimeText(owners.changed_at)}
        </p>
      )}
      {saved && !edited && <p role="status">Controlled organisations saved.</p>}
      {error && <p role="alert">The controlled organisations were not saved: {error}</p>}
    </SettingsCard>
  );
}
