// Renderer UI.
// Implements the right-sidebar module integration shell for right-sidebar embedded tools.
// Register available right-sidebar modules here instead of coupling them to the shell.

import { ClipboardCheck, Globe2, TerminalSquare } from "lucide-react";
import type { RightSidebarModuleDefinition, RightSidebarModuleId } from "./rightSidebarTypes";

export const RIGHT_SIDEBAR_MODULES: RightSidebarModuleDefinition[] = [
  {
    id: "review",
    titleKey: "rightSidebar.review",
    descriptionKey: "rightSidebar.reviewDescription",
    icon: ClipboardCheck,
    isEnabled: false,
  },
  {
    id: "terminal",
    titleKey: "rightSidebar.terminal",
    descriptionKey: "rightSidebar.terminalDescription",
    icon: TerminalSquare,
    isEnabled: true,
  },
  {
    id: "browser",
    titleKey: "rightSidebar.browser",
    descriptionKey: "rightSidebar.browserDescription",
    icon: Globe2,
    isEnabled: true,
  },
];

export function getRightSidebarModule(moduleId: RightSidebarModuleId) {
  return RIGHT_SIDEBAR_MODULES.find((module) => module.id === moduleId) ?? null;
}
