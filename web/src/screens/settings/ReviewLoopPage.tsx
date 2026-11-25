import { useCallback, useState } from "react";
import { useAssessmentSettings, useSetAssessmentSettings } from "../../api/assessment-settings";
import { ApiError } from "../../api/client";
import { useNow } from "../../app/now";
import { useNotifySetting } from "../../app/workNotifications";
import { messageOf } from "../../components/inspector/action";
import { AssessmentSettingsCard } from "../../components/settings/AssessmentSettingsCard";
import { NotificationsCard } from "../../components/settings/NotificationsCard";
import { Section } from "../../components/settings/Section";
import { StatusMessage } from "../../components/status/StatusMessage";
import { NotificationChannels } from "./NotificationChannels";
import { useReportDirty } from "./useReportDirty";

export function ReviewLoopPage({ onDirtyChange }: { onDirtyChange: (dirty: boolean) => void }) {
  const assessment = useAssessmentSettings();
  const save = useSetAssessmentSettings();
  const [saveCount, setSaveCount] = useState(0);
  const [dirty, setDirty] = useState({ assessment: false, ntfy: false, webhook: false });
  const assessmentDirty = useCallback(
    (value: boolean) => setDirty((current) => ({ ...current, assessment: value })),
    [],
  );
  const channelDirty = useCallback(
    (kind: "ntfy" | "webhook", value: boolean) => setDirty((current) => ({ ...current, [kind]: value })),
    [],
  );
  useReportDirty(dirty.assessment || dirty.ntfy || dirty.webhook, onDirtyChange);
  const now = new Date(useNow());
  const notify = useNotifySetting();
  return (
    <>
      {assessment.data ? (
        <AssessmentSettingsCard
          key={saveCount}
          settings={assessment.data}
          now={now}
          busy={save.isPending}
          saved={save.isSuccess}
          error={
            save.isError
              ? save.error instanceof ApiError && save.error.code === "conflict"
                ? "Settings changed elsewhere. Your edits are kept. Check the changed values and save again."
                : messageOf(save.error)
              : undefined
          }
          onSave={(config) =>
            save.mutate(
              { config, revision: assessment.data!.revision },
              { onSuccess: () => setSaveCount((count) => count + 1) },
            )
          }
          onDirtyChange={assessmentDirty}
        />
      ) : (
        <Section title="Comment assessment">
          <StatusMessage kind={assessment.isError ? "error" : "loading"}>
            {assessment.isError ? messageOf(assessment.error) : "Loading assessment settings…"}
          </StatusMessage>
        </Section>
      )}
      <section
        id="notifications"
        aria-labelledby="notifications-heading"
        className="grid min-w-0 scroll-mt-16 gap-4"
      >
        <h2 id="notifications-heading" className="text-base font-semibold">
          Notifications
        </h2>
        <NotificationsCard {...notify} />
        <NotificationChannels onDirtyChange={channelDirty} />
      </section>
    </>
  );
}
