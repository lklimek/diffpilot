import { useState } from "react";
import { useNavigate } from "react-router";
import { UserMenu } from "../../components/app-shell/UserMenu";
import { LLM_USAGE_PATH, SETTINGS_PATH } from "./paths";
import { useLlmActivity } from "./useLlmActivity";
import type { useSessionSignOut } from "./useSessionSignOut";

/** Keeps the account menu's clock and activity requests in the menu subtree. */
export function ShellUserMenu({
  login,
  onHelp,
  signOut,
}: {
  login: string;
  onHelp?: () => void;
  signOut: ReturnType<typeof useSessionSignOut>;
}) {
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const llm = useLlmActivity(login, open);
  return (
    <UserMenu
      {...llm}
      login={login}
      settingsHref={SETTINGS_PATH}
      usageHref={LLM_USAGE_PATH}
      onNavigate={(href) => void navigate(href)}
      onHelp={onHelp}
      onOpenChange={setOpen}
      onSignOut={() => signOut.leave(false)}
      signingOut={signOut.pending}
      signOutError={signOut.everywhere ? undefined : signOut.error}
    />
  );
}
