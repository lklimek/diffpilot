import { useId } from "react";
import { buttonClass } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";

/** Signed-out card: purpose, primary "Sign in with GitHub", and the promise never to submit a review. */
export function SignInCard({ loginHref }: { loginHref: string }) {
  const titleId = useId();
  return (
    <section
      className="grid gap-3 rounded-lg border border-border bg-surface px-6 py-5"
      aria-labelledby={titleId}
    >
      <h1 id={titleId} className="text-[22px] font-bold">
        diff<span className="text-accent">pilot</span>
      </h1>
      <p className="text-base text-muted">
        Review a GitHub pull request by what the change does, not by which lines moved.
      </p>
      <p>
        <Tooltip tip="Signs you in with your GitHub account. Only the GitHub logins this instance allows can sign in.">
          <a
            href={loginHref}
            data-variant="primary"
            className={`${buttonClass("primary")} px-5 py-2 text-base`}
          >
            Sign in with GitHub
          </a>
        </Tooltip>
      </p>
      <p className="text-xs text-muted">
        The tool reads the PR with your own GitHub access. It never submits a review for you.
      </p>
    </section>
  );
}
