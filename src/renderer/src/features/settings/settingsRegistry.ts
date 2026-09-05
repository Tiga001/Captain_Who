import type { LucideIcon } from 'lucide-react'
import {
  AppWindow,
  Archive,
  Bot,
  Cable,
  Clock,
  Gauge,
  Monitor,
  Settings,
  Shield,
  Sun,
  UserCircle,
  WandSparkles
} from 'lucide-react'
import type { TranslationKey } from '../../config/frontendTranslations'
import type { SettingsNode } from './settingsDefinition'
import { generalSettingsNodes } from './pages/GeneralSettingsPage.definition'
import { appearanceSettingsNodes } from './pages/AppearanceSettingsPage.definition'
import { profileSettingsNodes } from './pages/ProfileSettingsPage.definition'
import { personalizationSettingsNodes } from './pages/PersonalizationSettingsPage.definition'
import { usageBillingSettingsNodes } from './pages/UsageBillingSettingsPage.definition'
import { configurationSettings } from './pages/configuration/configuration.definition'
import {
  environmentSettings,
  skillsSettings,
  archivedConversationSettings,
  agentTemplateSettings
} from './pages/managementSettings.definition'
import { mcpSettings } from '../mcp/McpSettings.definition'
import { browserSettings } from '../mcp/BrowserAutomationSettings.definition'

export type SettingsPageId =
  | 'general'
  | 'profile'
  | 'appearance'
  | 'configuration'
  | 'personalization'
  | 'usageBilling'
  | 'skills'
  | 'mcp'
  | 'browser'
  | 'environment'
  | 'archivedConversations'
  | 'agentTemplates'

export interface SettingsNavItem {
  nodes: readonly SettingsNode[]
  id: SettingsPageId
  labelKey: TranslationKey
  icon: LucideIcon
}

export const SETTINGS_GROUPS: Array<{ titleKey: TranslationKey; items: SettingsNavItem[] }> = [
  {
    titleKey: 'settings.group.personal',
    items: [
      {
        id: 'general',
        nodes: generalSettingsNodes,
        labelKey: 'settings.page.general',
        icon: Settings
      },
      {
        id: 'profile',
        nodes: profileSettingsNodes,
        labelKey: 'settings.page.profile',
        icon: UserCircle
      },
      {
        id: 'appearance',
        nodes: appearanceSettingsNodes,
        labelKey: 'settings.page.appearance',
        icon: Sun
      },
      {
        id: 'configuration',
        nodes: configurationSettings,
        labelKey: 'settings.page.configuration',
        icon: Shield
      },
      {
        id: 'personalization',
        nodes: personalizationSettingsNodes,
        labelKey: 'settings.page.personalization',
        icon: Clock
      },
      {
        id: 'usageBilling',
        nodes: usageBillingSettingsNodes,
        labelKey: 'settings.page.usageBilling',
        icon: Gauge
      }
    ]
  },
  {
    titleKey: 'settings.group.coding',
    items: [
      {
        id: 'environment',
        nodes: environmentSettings,
        labelKey: 'settings.page.environment',
        icon: Monitor
      },
      { id: 'skills', nodes: skillsSettings, labelKey: 'settings.page.skills', icon: WandSparkles },
      { id: 'mcp', nodes: mcpSettings, labelKey: 'settings.nav.mcp', icon: Cable },
      { id: 'browser', nodes: browserSettings, labelKey: 'settings.nav.browser', icon: AppWindow },
      {
        id: 'agentTemplates',
        nodes: agentTemplateSettings,
        labelKey: 'settings.page.agentTemplates',
        icon: Bot
      }
    ]
  },
  {
    titleKey: 'settings.group.archived',
    items: [
      {
        id: 'archivedConversations',
        nodes: archivedConversationSettings,
        labelKey: 'settings.page.archivedConversations',
        icon: Archive
      }
    ]
  }
]
