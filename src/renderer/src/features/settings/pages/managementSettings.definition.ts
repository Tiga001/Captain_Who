import { defineSettingsNodes } from '../settingsDefinition'

export const environmentAddProjectSettings = defineSettingsNodes([
  { id: 'environment-add-project', title: 'environment.addProject' }
])
export const environmentEditProjectActions = defineSettingsNodes([
  {
    id: 'environment-edit-project',
    title: 'environment.editProject',
    prerequisiteId: 'environment-projects'
  }
])
export const environmentProjectActions = defineSettingsNodes([
  {
    id: 'environment-delete-project',
    title: 'environment.deleteProject',
    prerequisiteId: 'environment-projects'
  }
])
export const environmentSettings = defineSettingsNodes([
  {
    id: 'environment-projects',
    searchable: true,
    title: 'environment.selectProject',
    children: [
      ...environmentAddProjectSettings,
      ...environmentEditProjectActions,
      ...environmentProjectActions
    ]
  }
])

export const skillInstallSettings = defineSettingsNodes([
  { id: 'skills-install', title: 'skills.install', description: 'skills.pageDescription' }
])
export const skillManagementActions = defineSettingsNodes([
  { id: 'skills-update', title: 'skills.update', prerequisiteId: 'skills-management' },
  { id: 'skills-uninstall', title: 'skills.uninstall', prerequisiteId: 'skills-management' }
])
export const skillEnablementSettings = defineSettingsNodes([
  { id: 'skills-enabled', title: 'skills.toggleEnabledNamed', searchable: false }
])
export const skillManagementSettings = defineSettingsNodes([
  {
    id: 'skills-management',
    searchable: true,
    title: 'settings.page.skills',
    description: 'skills.pageDescription',
    terms: ['skills.sourceBundled', 'skills.sourceInstalled', 'skills.emptyDescription'],
    children: [...skillManagementActions, ...skillEnablementSettings]
  }
])
export const skillsSettings = defineSettingsNodes([
  ...skillInstallSettings,
  ...skillManagementSettings
])

export const archiveDeleteSettings = defineSettingsNodes([
  { id: 'archive-delete-all', title: 'archive.deleteAll' }
])
export const archiveProjectFilterSettings = defineSettingsNodes([
  {
    id: 'archive-project-filter',
    title: 'archive.projectFilter',
    terms: ['archive.allProjects', 'archive.noProject']
  }
])
export const archiveConversationDeleteSettings = defineSettingsNodes([
  {
    id: 'archive-delete-conversation',
    title: 'archive.deleteConversation',
    prerequisiteId: 'archive-conversations'
  }
])
export const archiveConversationRestoreSettings = defineSettingsNodes([
  {
    id: 'archive-unarchive-conversation',
    title: 'archive.unarchive',
    prerequisiteId: 'archive-conversations'
  }
])
export const archiveConversationListSettings = defineSettingsNodes([
  {
    id: 'archive-conversations',
    searchable: true,
    title: 'settings.page.archivedConversations',
    children: [...archiveConversationDeleteSettings, ...archiveConversationRestoreSettings]
  }
])
export const archivedConversationSettings = defineSettingsNodes([
  ...archiveDeleteSettings,
  ...archiveProjectFilterSettings,
  ...archiveConversationListSettings
])

export const agentTemplateFormSettings = defineSettingsNodes([
  { id: 'agent-template-name', title: 'agentTemplates.name' },
  {
    id: 'agent-template-projects',
    title: 'agentTemplates.projects',
    description: 'agentTemplates.projectsHint'
  },
  {
    id: 'agent-template-description',
    title: 'agentTemplates.description',
    description: 'agentTemplates.descriptionPlaceholder'
  },
  {
    id: 'agent-template-instructions',
    title: 'agentTemplates.instructions',
    description: 'agentTemplates.instructionsPlaceholder'
  },
  {
    id: 'agent-template-model',
    title: 'agentTemplates.model',
    terms: ['agentTemplates.selectModel']
  }
])
export const agentTemplateEditorSettings = defineSettingsNodes([
  {
    id: 'agent-templates-editor',
    searchable: true,
    title: 'agentTemplates.edit',
    view: 'editor',
    prerequisiteId: 'agent-templates-list',
    children: agentTemplateFormSettings
  }
])
export const agentTemplateCreateSettings = defineSettingsNodes([
  { id: 'agent-templates-create', title: 'agentTemplates.create', view: 'templates' }
])
export const agentTemplateRowSettings = defineSettingsNodes([
  {
    id: 'agent-templates-enabled',
    title: 'agentTemplates.enable',
    terms: ['agentTemplates.disable'],
    prerequisiteId: 'agent-templates-list'
  },
  {
    id: 'agent-templates-delete',
    title: 'agentTemplates.delete',
    prerequisiteId: 'agent-templates-list'
  }
])
export const agentTemplateLibrarySettings = defineSettingsNodes([
  {
    id: 'agent-templates-list',
    searchable: true,
    title: 'agentTemplates.list',
    description: 'agentTemplates.descriptionText',
    view: 'templates',
    children: [...agentTemplateEditorSettings, ...agentTemplateRowSettings]
  }
])
export const agentCollaborationSettingsNodes = defineSettingsNodes([
  {
    id: 'agent-collaboration-enabled',
    title: 'agentTemplates.collaborationEnabled',
    description: 'agentTemplates.collaborationEnabledDescription',
    view: 'templates'
  }
])
export const agentTemplateSettings = defineSettingsNodes([
  ...agentCollaborationSettingsNodes,
  ...agentTemplateCreateSettings,
  ...agentTemplateLibrarySettings
])
