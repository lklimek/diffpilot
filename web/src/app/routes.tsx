import type { RouteObject } from "react-router";
import { ShellRoute, StartRoute } from "../screens/shell/ShellRoute";
import { HomeScreen } from "../screens/home/HomeScreen";
import { NotFoundScreen } from "../screens/home/NotFoundScreen";
import { ProgressScreen } from "../screens/progress/ProgressScreen";
import { SettingsHelpScreen } from "../screens/settings/SettingsHelpScreen";
import { SettingsScreen } from "../screens/settings/SettingsScreen";
import { TourScreen } from "../screens/tour/TourScreen";
import { FilesScreen } from "../screens/files/FilesScreen";
import { ReviewRoute } from "../screens/review/ReviewRoute";

/** App routes (plan §F.1): My work, the review start page and Settings share the navigation shell. */
export const appRoutes: RouteObject[] = [
  {
    element: <ShellRoute />,
    children: [
      { path: "/", element: <StartRoute /> },
      { path: "/review", element: <HomeScreen /> },
      { path: "/settings", element: <SettingsScreen /> },
      { path: "/settings/help", element: <SettingsHelpScreen /> },
    ],
  },
  {
    path: "/analyses/:analysisId",
    element: <ReviewRoute />,
    children: [
      { index: true, element: <TourScreen /> },
      { path: "files", element: <FilesScreen /> },
    ],
  },
  { path: "/analyses/:analysisId/progress", element: <ProgressScreen /> },
  { path: "*", element: <NotFoundScreen /> },
];
