import { useEffect, useState, type ReactNode, type ComponentProps } from "react";
import type { DashboardSettings, DashboardSettingsView } from "../../../api/generated/types";
import { busyTip, buttonClass } from "../../button/button";
import { useAction } from "../../inspector/action";
import { Section } from "../../settings/Section";
import { Tooltip } from "../../tooltip/Tooltip";
import { ListEditor } from "./ListEditor";
import { MemcanGroup } from "./MemcanGroup";
import { RepositoryTable } from "./RepositoryTable";
import { RulesGroup } from "./RulesGroup";
import { ScheduleGroup } from "./ScheduleGroup";
import { SlackNamesTable } from "./SlackNamesTable";
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

interface DashboardSettingsCardProps {
  /** The stored settings, their defaults and what the server derived from them. */
  view: DashboardSettingsView;
  page: "github-sync" | "repositories";
  repositoryOptions?: Pick<
    ComponentProps<typeof RepositoryTable>,
    "installations" | "installationStatusKnown" | "expanded" | "onExpand" | "details"
  >;
  children?: ReactNode;
  /** Saves the whole form (one `PUT`); rejects with the server's message (422 names the field). */
  onSave: (settings: DashboardSettings) => Promise<unknown>;
  /** Called when the form starts or stops differing from the stored settings. */
  onDirtyChange?: (dirty: boolean) => void;
}

/** Per-user dashboard settings of plan §F.6, saved together with Save. */
export function DashboardSettingsCard({
  view,
  page,
  repositoryOptions,
  children,
  onSave,
  onDirtyChange,
}: DashboardSettingsCardProps) {
  const [draft, setDraft] = useState<SettingsDraft>(() => toDraft(view.settings));
  const [attempted, setAttempted] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const action = useAction();
  const own = view.own_login;
  const settings = () => {
    const edited = fromDraft(draft);
    return page === "repositories"
      ? { ...view.settings, repositories: edited.repositories }
      : { ...edited, repositories: view.settings.repositories };
  };
  const errors = attempted ? validateSettings(settings(), own) : {};
  const problems = Object.values(errors);
  const dirty = !sameSettings(settings(), view.settings);
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
    const next = settings();
    setAttempted(true);
    setNote(null);
    if (Object.keys(validateSettings(next, own)).length > 0) return;
    if (await action.run(() => onSave(next))) setNote("Dashboard settings saved.");
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
    <>
      <fieldset disabled={action.busy} className="grid min-w-0 gap-4">
        {page === "repositories" ? (
          <Section title="Repository rules">
            <RepositoryTable
              rows={draft.repositories}
              errors={errors}
              unreachable={view.unreachable}
              onChange={(repositories) => edit({ repositories })}
              {...repositoryOptions}
            />
          </Section>
        ) : (
          <>
            <Section title="Scope">
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
              <RulesGroup
                lookbackDays={draft.lookback_days}
                readyLabel={draft.ready_label}
                skipAuthors={draft.skip_authors}
                knownBots={draft.known_bots}
                errors={errors}
                checkAuthor={check("skip_authors")}
                checkBot={check("known_bots")}
                onChange={edit}
              />
            </Section>
            <Section title="Nightly scan">
              <ScheduleGroup
                enabled={draft.schedule_enabled}
                time={draft.schedule_time}
                timeZone={draft.time_zone}
                errors={errors}
                onChange={edit}
              />
            </Section>
            <Section title="TODO sources">
              <MemcanGroup
                includeTodos={draft.include_todos}
                configured={view.memcan_configured}
                repoProjects={repoProjects}
                extraProjects={draft.memcan_extra_projects}
                errors={errors}
                checkProject={check("memcan_extra_projects")}
                onChange={edit}
              />
            </Section>
            <Section title="Slack names">
              <SlackNamesTable
                rows={draft.slack_names}
                errors={errors}
                onChange={(slack_names) => edit({ slack_names })}
              />
            </Section>
          </>
        )}
      </fieldset>
      {children}
      <div className="flex flex-wrap justify-end gap-2">
        {dirty && <span className="self-center text-sm font-semibold text-med">Unsaved changes</span>}
        {page === "github-sync" && (
          <Tooltip
            tip={busyTip(
              "Fills this page with the defaults, keeping your time zone. Save to apply.",
              action.busy,
            )}
          >
            <button
              type="button"
              className={buttonClass("plain")}
              onClick={() => !action.busy && reset()}
              aria-disabled={action.busy || undefined}
            >
              Reset to defaults
            </button>
          </Tooltip>
        )}
        <Tooltip tip={busyTip("Saves the changes on this page.", action.busy)}>
          <button
            type="button"
            aria-disabled={action.busy || undefined}
            className={buttonClass("primary")}
            onClick={() => void save()}
          >
            {action.busy ? "Saving…" : "Save"}
          </button>
        </Tooltip>
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
    </>
  );
}
