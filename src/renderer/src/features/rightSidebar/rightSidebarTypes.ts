// Renderer UI.
// Defines the right-sidebar module integration shell and keep-alive page model.
// Right-sidebar pages may host terminal, review, file, browser, or future embedded modules.

import type { LucideIcon } from "lucide-react";
import type { TranslationKey } from "../../config/frontendTranslations";

export type RightSidebarModuleId = "review" | "terminal" | "browser";

export interface RightSidebarModuleDefinition {
  id: RightSidebarModuleId;
  titleKey: TranslationKey;
  descriptionKey: TranslationKey;
  icon: LucideIcon;
  isEnabled: boolean;
}

export interface RightSidebarPage {
  iconUrl?: string | null;
  id: string;
  moduleId: RightSidebarModuleId;
  title: string;
  workspaceKey?: string | null;
  workspacePath?: string;
}
