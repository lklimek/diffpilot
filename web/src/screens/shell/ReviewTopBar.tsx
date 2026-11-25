import { useNavigate } from "react-router";
import type { ComponentProps } from "react";
import { TopBar } from "../../components/top-bar/TopBar";
import { useSessionSignOut } from "./useSessionSignOut";
import { PRS_PATH } from "./paths";
import { ShellUserMenu } from "./ShellUserMenu";

/** The review top bar with the same account controls as the navigation shell. */
export function ReviewTopBar({
  login,
  onHelp,
  ...props
}: Omit<ComponentProps<typeof TopBar>, "userMenu" | "prsHref" | "onNavigate"> & {
  login: string | null;
  onHelp?: () => void;
}) {
  const navigate = useNavigate();
  const signOut = useSessionSignOut(login);
  return (
    <TopBar
      {...props}
      prsHref={PRS_PATH}
      onNavigate={(href) => void navigate(href)}
      userMenu={login && <ShellUserMenu key={login} login={login} onHelp={onHelp} signOut={signOut} />}
    />
  );
}
