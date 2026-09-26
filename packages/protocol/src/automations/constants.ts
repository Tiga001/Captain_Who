/** Stable JSON-RPC contract shared by Renderer, Electron Main, and Core. */
export const AUTOMATION_SCHEMA_VERSION = 1 as const

export const AUTOMATION_PERMISSION_MODE_VERSION = 2 as const

export const AUTOMATION_ERROR_CODE = -32045 as const

export const AUTOMATION_LIST_METHOD = 'automation.list'

export const AUTOMATION_GET_METHOD = 'automation.get'

export const AUTOMATION_CREATE_METHOD = 'automation.create'

export const AUTOMATION_UPDATE_METHOD = 'automation.update'

export const AUTOMATION_SET_ENABLED_METHOD = 'automation.setEnabled'

export const AUTOMATION_RUN_NOW_METHOD = 'automation.runNow'

export const AUTOMATION_DELETE_METHOD = 'automation.delete'

export const AUTOMATION_RUNS_LIST_METHOD = 'automation.runs.list'

export const AUTOMATION_ATTENTION_SUMMARY_METHOD = 'automation.attention.summary'

export const AUTOMATION_ATTENTION_ACKNOWLEDGE_METHOD = 'automation.attention.acknowledge'

export const AUTOMATION_EVENT_NOTIFICATION_METHOD = 'automation.event'

export const AUTOMATION_RESYNC_NOTIFICATION_METHOD = 'automation.resync'
