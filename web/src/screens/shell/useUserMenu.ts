import { useNavigate } from "react-router";
import type { UserMenuProps } from "../../components/app-shell/UserMenu";
import { LLM_USAGE_PATH, SETTINGS_PATH } from "./paths";
import { useLlmActivity } from "./useLlmActivity";
import { useSessionSignOut } from "./useSessionSignOut";

/** Supplies the signed-in controls of both top bars. */
export function useUserMenu(login: string | null, onHelp?: () => void) {
  const navigate = useNavigate();
  const signOut = useSessionSignOut(login);
  const llm = useLlmActivity(login);
  const userMenu: UserMenuProps | undefined = login
    ? {
        ...llm,
        login,
        settingsHref: SETTINGS_PATH,
        usageHref: LLM_USAGE_PATH,
        onNavigate: (href) => void navigate(href),
        onHelp,
        onSignOut: () => signOut.leave(false),
        signingOut: signOut.pending,
        signOutError: signOut.everywhere ? undefined : signOut.error,
      }
    : undefined;
  return { userMenu, signOut };
}
