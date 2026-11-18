import { useEffect, useState, type ReactNode } from "react";
import type { DashboardSettings, DashboardSettingsView } from "../../../api/generated/types";
import { busyTip, buttonClass } from "../../button/button";
import { useAction } from "../../inspector/action";
import { SettingsCard } from "../../settings/Card";
import { Tooltip } from "../../tooltip/Tooltip";
import { ListEditor } from "./ListEditor";
import { MemcanGroup } from "./MemcanGroup";
import { RepositoryTable } from "./RepositoryTable";
import { RulesGroup } from "./RulesGroup";
import { ScheduleGroup } from "./ScheduleGroup";
import { UnreachableMark, isUnreachable } from "./UnreachableMark";
import {
  checkEntry,
  fromDraft,
  sameSettings,
  toDraft,
  validateSettings,
  type ListField,
  type SettingsDraft,
} from "./validation";
import { HelpTip } from "../../settings/HelpTip";
import { SEARCH_REFUSED_GROUP } from "../../settings/settingsHelp";

function Group({ legend, children }: { legend: string; children: ReactNode }) {
  return (
    <fieldset className="grid min-w-0 content-start gap-2 rounded-md border border-border px-3 pt-1 pb-3">
      <legend className="px-1 text-sm font-semibold">{legend}</legend>
      {children}
    </fieldset>
  );
}

interface DashboardSettingsCardProps {
  /** The stored settings, their defaults and what the server derived from them. */
  view: DashboardSettingsView;
  /** Saves the whole form (one `PUT`); rejects with the server's message (422 names the field). */
  onSave: (settings: DashboardSettings) => Promise<unknown>;
  /** Called when the form starts or stops differing from the stored settings. */
  onDirtyChange?: (dirty: boolean) => void;
}

/** Per-user dashboard settings of plan §F.6, saved together with Save. */
export function DashboardSettingsCard({ view, onSave, onDirtyChange }: DashboardSettingsCardProps) {
  const [draft, setDraft] = useState<SettingsDraft>(() => toDraft(view.settings));
  const [attempted, setAttempted] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const action = useAction();
  const own = view.own_login;
  const errors = attempted ? validateSettings(fromDraft(draft), own) : {};
  const problems = Object.values(errors);
  const dirty = !sameSettings(fromDraft(draft), view.settings);
  useEffect(() => onDirtyChange?.(dirty), [dirty, onDirtyChange]);
  useEffect(() => () => onDirtyChange?.(false), [onDirtyChange]);
  const edit = (change: Partial<SettingsDraft>) => {
    setDraft((d) => ({ ...d, ...change }));
    setNote(null);
    if (action.error) action.setError(null);
  };
  const check = (field: ListField) => (value: string, items: string[]) =>
    checkEntry(field, value, items, own);
  const save = async () => {
    if (action.busy) return;
    const settings = fromDraft(draft);
    setAttempted(true);
    setNote(null);
    if (Object.keys(validateSettings(settings, own)).length > 0) return;
    if (await action.run(() => onSave(settings))) setNote("Dashboard settings saved.");
  };
  const reset = () => {
    setDraft((d) => toDraft({ ...view.defaults, time_zone: d.time_zone === "" ? null : d.time_zone }));
    setAttempted(false);
    setNote("Defaults filled in. Save to keep them.");
  };
  const repoProjects = [
    ...new Set(draft.repositories.map((r) => r.repo.split("/")[1]?.trim() ?? "").filter((n) => n !== "")),
  ];
  return (
    <SettingsCard title="Dashboard">
      <p className="text-xs text-muted">
        Your own settings for My work: what the scan reads and how items are ranked.
      </p>
      <Group legend="Accounts">
        <p className="text-sm">
          Your login <code className="font-mono">{own}</code> is always scanned.
        </p>
        <ListEditor
          label="Linked logins"
          help={<HelpTip topic="linked_logins" />}
          hint="Other GitHub accounts of yours, such as a bot account that opens pull requests for you."
          addLabel="Add a linked login"
          items={draft.linked_logins}
          empty="None. Add another GitHub login of yours to scan it too."
          placeholder="second-login"
          check={check("linked_logins")}
          error={errors.linked_logins}
          onChange={(linked_logins) => edit({ linked_logins })}
        />
      </Group>
      <Group legend="Repositories">
        <RepositoryTable
          rows={draft.repositories}
          errors={errors}
          unreachable={view.unreachable}
          onChange={(repositories) => edit({ repositories })}
        />
      </Group>
      <div className="grid items-start gap-3 lg:grid-cols-2">
        <Group legend={SEARCH_REFUSED_GROUP}>
          <ListEditor
            label="Owners to list"
            help={<HelpTip topic="enumerate_owners" />}
            hint="Optional. Organisations or users whose open pull requests and issues are listed directly when GitHub search will not answer for one of your accounts. Example: dashpay"
            addLabel="Add an owner to list"
            items={draft.enumerate_owners}
            empty="None."
            placeholder="organisation"
            check={check("enumerate_owners")}
            error={errors.enumerate_owners}
            mark={(owner) => isUnreachable(view.unreachable, owner) && <UnreachableMark name={owner} />}
            onChange={(enumerate_owners) => edit({ enumerate_owners })}
          />
          <p className="text-xs text-muted">
            When GitHub search refuses a login, the open pull requests and issues of these owners are listed
            instead.
          </p>
        </Group>
        <Group legend="Rules">
          <RulesGroup
            lookbackDays={draft.lookback_days}
            readyLabel={draft.ready_label}
            skipAuthors={draft.skip_authors}
            errors={errors}
            checkAuthor={check("skip_authors")}
            onChange={edit}
          />
        </Group>
        <Group legend="MemCan TODOs">
          <MemcanGroup
            includeTodos={draft.include_todos}
            configured={view.memcan_configured}
            repoProjects={repoProjects}
            extraProjects={draft.memcan_extra_projects}
            errors={errors}
            checkProject={check("memcan_extra_projects")}
            onChange={edit}
          />
        </Group>
        <Group legend="Automatic refresh">
          <ScheduleGroup
            enabled={draft.schedule_enabled}
            time={draft.schedule_time}
            timeZone={draft.time_zone}
            errors={errors}
            onChange={edit}
          />
        </Group>
      </div>
      <div className="flex flex-wrap gap-2">
        <Tooltip tip={busyTip("Saves these dashboard settings; the next refresh uses them.", action.busy)}>
          <button
            type="button"
            aria-disabled={action.busy || undefined}
            className={buttonClass("primary")}
            onClick={() => void save()}
          >
            {action.busy ? "Saving…" : "Save dashboard settings"}
          </button>
        </Tooltip>
        <Tooltip
          tip={busyTip(
            "Fills the form with the defaults, keeping your time zone. Save to apply.",
            action.busy,
          )}
        >
          <button
            type="button"
            aria-disabled={action.busy || undefined}
            className={buttonClass("plain")}
            onClick={() => !action.busy && reset()}
          >
            Reset to defaults
          </button>
        </Tooltip>
        {dirty && <span className="self-center text-sm font-semibold text-med">Unsaved changes</span>}
      </div>
      {problems.length > 0 && (
        <div role="alert" className="text-sm text-high">
          <p>Not saved: fix the marked fields.</p>
          <ul className="list-disc pl-5">
            {problems.map((p) => (
              <li key={p}>{p}</li>
            ))}
          </ul>
        </div>
      )}
      {action.error && (
        <p role="alert" className="text-sm text-high">
          Not saved: {action.error}
        </p>
      )}
      {note && <p role="status">{note}</p>}
    </SettingsCard>
  );
}
