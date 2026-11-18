import type { RouteObject } from "react-router";
import { ShellRoute, StartRoute } from "../screens/dashboard/ShellRoute";
import { HomeScreen } from "../screens/home/HomeScreen";
import { NotFoundScreen } from "../screens/home/NotFoundScreen";
import { ProgressScreen } from "../screens/progress/ProgressScreen";
import { SettingsScreen } from "../screens/settings/SettingsScreen";
import { TourScreen } from "../screens/tour/TourScreen";

/** App routes (plan §F.1): My work, the review start page and Settings share the navigation shell. */
export const appRoutes: RouteObject[] = [
  {
    element: <ShellRoute />,
    children: [
      { path: "/", element: <StartRoute /> },
      { path: "/review", element: <HomeScreen /> },
      { path: "/settings", element: <SettingsScreen /> },
    ],
  },
  { path: "/analyses/:analysisId", element: <TourScreen /> },
  { path: "/analyses/:analysisId/progress", element: <ProgressScreen /> },
  { path: "*", element: <NotFoundScreen /> },
];
