import type { RouteObject } from "react-router";
import { HomeScreen } from "../screens/home/HomeScreen";
import { NotFoundScreen } from "../screens/home/NotFoundScreen";
import { ProgressScreen } from "../screens/progress/ProgressScreen";
import { SettingsScreen } from "../screens/settings/SettingsScreen";
import { TourScreen } from "../screens/tour/TourScreen";

/** App routes; add-only (plan §4.3). */
export const appRoutes: RouteObject[] = [
  { path: "/", element: <HomeScreen /> },
  { path: "/analyses/:analysisId", element: <TourScreen /> },
  { path: "/analyses/:analysisId/progress", element: <ProgressScreen /> },
  { path: "/settings", element: <SettingsScreen /> },
  // Dark launch (plan §H.2.2): unlinked until WP11-SWITCH makes My work the start page.
  // TODO(WP11-WEB-S after WEB-C merge): import DashboardScreen statically like the other screens.
  {
    path: "/my-work",
    lazy: async () => ({ Component: (await import("../screens/dashboard/DashboardScreen")).DashboardScreen }),
  },
  { path: "*", element: <NotFoundScreen /> },
];
