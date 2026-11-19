import { Fragment } from "react";
import { GithubMark } from "../github/GithubMark";
import { isGithubUrl } from "../github/links";
import { HELP_SECTIONS, HELP_TOPICS, SETTINGS_HELP, helpLines } from "./settingsHelp";

const URL_TEXT = /(https:\/\/\S+[^\s.,;:)])/;

/** `text` with its https addresses as links (new tab). */
function Linked({ text }: { text: string }) {
  return (
    <>
      {text.split(URL_TEXT).map((part, i) =>
        i % 2 === 1 ? (
          <a key={i} href={part} target="_blank" rel="noopener noreferrer" className="text-accent underline">
            {part}
            {isGithubUrl(part) && <GithubMark />}
          </a>
        ) : (
          <Fragment key={i}>{part}</Fragment>
        ),
      )}
    </>
  );
}

/** The whole settings guide (docs/settings.md in the app): every option by section, each with its anchor. */
export function SettingsHelpGuide() {
  return (
    <div className="grid max-w-[100ch] gap-6">
      {HELP_SECTIONS.map((section) => (
        <div key={section} className="grid gap-3">
          <h2 className="text-lg font-semibold">{section}</h2>
          {HELP_TOPICS.filter((topic) => SETTINGS_HELP[topic].section === section).map((topic) => {
            const help = SETTINGS_HELP[topic];
            return (
              <section
                key={topic}
                id={topic}
                aria-labelledby={`${topic}-title`}
                className="grid scroll-mt-16 gap-1.5 rounded-lg border border-border bg-surface px-4 py-3 target:border-accent"
              >
                <h3 id={`${topic}-title`} className="text-base font-semibold">
                  {help.title}
                </h3>
                <dl className="m-0 grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-sm">
                  {helpLines(help).map(([label, text]) => (
                    <Fragment key={label}>
                      <dt className="font-semibold text-muted">{label}</dt>
                      <dd className="m-0">
                        <Linked text={text} />
                      </dd>
                    </Fragment>
                  ))}
                </dl>
                {help.details?.map((paragraph) => (
                  <p key={paragraph} className="text-sm">
                    <Linked text={paragraph} />
                  </p>
                ))}
              </section>
            );
          })}
        </div>
      ))}
    </div>
  );
}
