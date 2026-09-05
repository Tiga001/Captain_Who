import { defineSettingsNodes } from '../settings/settingsDefinition'

export const mcpEditorSettings = defineSettingsNodes([
  { id: 'mcp-server-name', title: 'mcp.form.name' },
  {
    id: 'mcp-server-transport',
    title: 'mcp.form.transport',
    terms: ['mcp.form.localProcess', { text: 'STDIO' }]
  },
  {
    id: 'mcp-server-executable',
    title: 'mcp.form.executable',
    terms: ['mcp.form.chooseExecutable']
  },
  { id: 'mcp-server-arguments', title: 'mcp.form.arguments', terms: ['mcp.form.addArgument'] },
  { id: 'mcp-server-cwd', title: 'mcp.form.cwd', terms: ['mcp.form.chooseCwd'] },
  {
    id: 'mcp-server-auto-execute',
    title: 'mcp.form.autoExecute',
    description: 'mcp.form.toolApprovalHelp',
    terms: ['mcp.form.promptEveryCall', 'mcp.form.approvalMode']
  }
])

export const mcpHeaderSettings = defineSettingsNodes([
  { id: 'mcp-refresh', title: 'mcp.actions.refresh', view: 'list' },
  { id: 'mcp-add-server', title: 'mcp.actions.addServer', view: 'list' }
])
export const mcpEditorSectionSettings = defineSettingsNodes([
  {
    id: 'mcp-server-editor',
    searchable: true,
    title: 'mcp.edit.title',
    view: 'editor',
    prerequisiteId: 'mcp-servers',
    children: mcpEditorSettings
  }
])
export const mcpManagementSettings = defineSettingsNodes([
  {
    id: 'mcp-servers',
    searchable: true,
    title: 'settings.page.mcp',
    description: 'mcp.page.description',
    view: 'list',
    children: mcpEditorSectionSettings
  }
])
export const mcpSettings = defineSettingsNodes([...mcpHeaderSettings, ...mcpManagementSettings])
