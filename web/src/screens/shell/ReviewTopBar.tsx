import type { ComponentProps } from "react";
import { TopBar } from "../../components/top-bar/TopBar";
import { useUserMenu } from "./useUserMenu";

/** The review top bar with the same account controls as the navigation shell. */
export function ReviewTopBar({
  login,
  onHelp,
  ...props
}: Omit<ComponentProps<typeof TopBar>, "userMenu"> & {
  login: string | null;
  onHelp?: () => void;
}) {
  const { userMenu } = useUserMenu(login, onHelp);
  return <TopBar {...props} userMenu={userMenu} />;
}
