import {
  parseBrowserArtifactToolProjection,
  type AgentToolIdentity,
  type AgentToolResult,
  type BrowserArtifactReference
} from '@mycopilot/protocol'
import { Ban, CheckCircle2, CircleAlert, LoaderCircle, XCircle } from 'lucide-react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { toSafeMcpDisplayText } from '../../../mcp/mcpSafeDisplay'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'
import { BrowserArtifactCards } from './BrowserArtifactCards'

type BuiltinCapabilityIdentity = Extract<AgentToolIdentity, { type: 'builtin_capability' }>
type BrowserToolStatus = 'running' | 'completed' | 'failed' | 'cancelled' | 'outcomeUnknown'

const browserToolFamilyById = {
  browser_annotate: 'devtools',
  browser_click: 'click',
  browser_close: 'close',
  browser_console_messages: 'console',
  browser_cookie_clear: 'storageWrite',
  browser_cookie_delete: 'storageWrite',
  browser_cookie_get: 'storageRead',
  browser_cookie_list: 'storageRead',
  browser_cookie_set: 'storageWrite',
  browser_drag: 'drag',
  browser_drop: 'drag',
  browser_evaluate: 'script',
  browser_file_upload: 'upload',
  browser_fill_form: 'fillForm',
  browser_find: 'find',
  browser_generate_locator: 'locator',
  browser_get_config: 'config',
  browser_handle_dialog: 'dialog',
  browser_hide_highlight: 'devtools',
  browser_highlight: 'devtools',
  browser_hover: 'hover',
  browser_localstorage_clear: 'storageWrite',
  browser_localstorage_delete: 'storageWrite',
  browser_localstorage_get: 'storageRead',
  browser_localstorage_list: 'storageRead',
  browser_localstorage_set: 'storageWrite',
  browser_mouse_click_xy: 'pageInteraction',
  browser_mouse_down: 'pageInteraction',
  browser_mouse_drag_xy: 'pageInteraction',
  browser_mouse_move_xy: 'pageInteraction',
  browser_mouse_up: 'pageInteraction',
  browser_mouse_wheel: 'pageInteraction',
  browser_navigate: 'navigate',
  browser_navigate_back: 'navigateBack',
  browser_network_request: 'networkRead',
  browser_network_requests: 'networkRead',
  browser_network_state_set: 'networkConfigure',
  browser_pdf_save: 'pdf',
  browser_press_key: 'pressKey',
  browser_resize: 'resize',
  browser_resume: 'resume',
  browser_route: 'networkConfigure',
  browser_route_list: 'networkRules',
  browser_run_code_unsafe: 'script',
  browser_select_option: 'selectOption',
  browser_sessionstorage_clear: 'storageWrite',
  browser_sessionstorage_delete: 'storageWrite',
  browser_sessionstorage_get: 'storageRead',
  browser_sessionstorage_list: 'storageRead',
  browser_sessionstorage_set: 'storageWrite',
  browser_set_storage_state: 'storageWrite',
  browser_snapshot: 'snapshot',
  browser_start_tracing: 'recordStart',
  browser_start_video: 'recordStart',
  browser_stop_tracing: 'recordStop',
  browser_stop_video: 'recordStop',
  browser_storage_state: 'storageRead',
  browser_tabs: 'tabs',
  browser_take_screenshot: 'screenshot',
  browser_type: 'type',
  browser_unroute: 'networkConfigure',
  browser_verify_element_visible: 'verify',
  browser_verify_list_visible: 'verify',
  browser_verify_text_visible: 'verify',
  browser_verify_value: 'verify',
  browser_video_chapter: 'recordEdit',
  browser_video_hide_actions: 'recordEdit',
  browser_video_show_actions: 'recordEdit',
  browser_wait_for: 'waitFor'
} as const

type BrowserToolFamily = (typeof browserToolFamilyById)[keyof typeof browserToolFamilyById]

interface BuiltinCapabilityToolActivityProps {
  cancelled?: boolean
  displayReason?: string | null
  identity: BuiltinCapabilityIdentity
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

const browserToolStatusKeys = {
  navigate: {
    running: 'agent.builtinCapability.browser.navigate.running',
    completed: 'agent.builtinCapability.browser.navigate.completed',
    failed: 'agent.builtinCapability.browser.navigate.failed',
    cancelled: 'agent.builtinCapability.browser.navigate.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.navigate.outcomeUnknown'
  },
  snapshot: {
    running: 'agent.builtinCapability.browser.snapshot.running',
    completed: 'agent.builtinCapability.browser.snapshot.completed',
    failed: 'agent.builtinCapability.browser.snapshot.failed',
    cancelled: 'agent.builtinCapability.browser.snapshot.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.snapshot.outcomeUnknown'
  },
  find: {
    running: 'agent.builtinCapability.browser.find.running',
    completed: 'agent.builtinCapability.browser.find.completed',
    failed: 'agent.builtinCapability.browser.find.failed',
    cancelled: 'agent.builtinCapability.browser.find.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.find.outcomeUnknown'
  },
  click: {
    running: 'agent.builtinCapability.browser.click.running',
    completed: 'agent.builtinCapability.browser.click.completed',
    failed: 'agent.builtinCapability.browser.click.failed',
    cancelled: 'agent.builtinCapability.browser.click.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.click.outcomeUnknown'
  },
  type: {
    running: 'agent.builtinCapability.browser.type.running',
    completed: 'agent.builtinCapability.browser.type.completed',
    failed: 'agent.builtinCapability.browser.type.failed',
    cancelled: 'agent.builtinCapability.browser.type.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.type.outcomeUnknown'
  },
  fillForm: {
    running: 'agent.builtinCapability.browser.fillForm.running',
    completed: 'agent.builtinCapability.browser.fillForm.completed',
    failed: 'agent.builtinCapability.browser.fillForm.failed',
    cancelled: 'agent.builtinCapability.browser.fillForm.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.fillForm.outcomeUnknown'
  },
  pressKey: {
    running: 'agent.builtinCapability.browser.pressKey.running',
    completed: 'agent.builtinCapability.browser.pressKey.completed',
    failed: 'agent.builtinCapability.browser.pressKey.failed',
    cancelled: 'agent.builtinCapability.browser.pressKey.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.pressKey.outcomeUnknown'
  },
  tabs: {
    running: 'agent.builtinCapability.browser.tabs.running',
    completed: 'agent.builtinCapability.browser.tabs.completed',
    failed: 'agent.builtinCapability.browser.tabs.failed',
    cancelled: 'agent.builtinCapability.browser.tabs.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.tabs.outcomeUnknown'
  },
  waitFor: {
    running: 'agent.builtinCapability.browser.waitFor.running',
    completed: 'agent.builtinCapability.browser.waitFor.completed',
    failed: 'agent.builtinCapability.browser.waitFor.failed',
    cancelled: 'agent.builtinCapability.browser.waitFor.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.waitFor.outcomeUnknown'
  },
  close: {
    running: 'agent.builtinCapability.browser.close.running',
    completed: 'agent.builtinCapability.browser.close.completed',
    failed: 'agent.builtinCapability.browser.close.failed',
    cancelled: 'agent.builtinCapability.browser.close.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.close.outcomeUnknown'
  },
  pageInteraction: {
    running: 'agent.builtinCapability.browser.pageInteraction.running',
    completed: 'agent.builtinCapability.browser.pageInteraction.completed',
    failed: 'agent.builtinCapability.browser.pageInteraction.failed',
    cancelled: 'agent.builtinCapability.browser.pageInteraction.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.pageInteraction.outcomeUnknown'
  },
  hover: {
    running: 'agent.builtinCapability.browser.hover.running',
    completed: 'agent.builtinCapability.browser.hover.completed',
    failed: 'agent.builtinCapability.browser.hover.failed',
    cancelled: 'agent.builtinCapability.browser.hover.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.hover.outcomeUnknown'
  },
  selectOption: {
    running: 'agent.builtinCapability.browser.selectOption.running',
    completed: 'agent.builtinCapability.browser.selectOption.completed',
    failed: 'agent.builtinCapability.browser.selectOption.failed',
    cancelled: 'agent.builtinCapability.browser.selectOption.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.selectOption.outcomeUnknown'
  },
  drag: {
    running: 'agent.builtinCapability.browser.drag.running',
    completed: 'agent.builtinCapability.browser.drag.completed',
    failed: 'agent.builtinCapability.browser.drag.failed',
    cancelled: 'agent.builtinCapability.browser.drag.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.drag.outcomeUnknown'
  },
  dialog: {
    running: 'agent.builtinCapability.browser.dialog.running',
    completed: 'agent.builtinCapability.browser.dialog.completed',
    failed: 'agent.builtinCapability.browser.dialog.failed',
    cancelled: 'agent.builtinCapability.browser.dialog.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.dialog.outcomeUnknown'
  },
  navigateBack: {
    running: 'agent.builtinCapability.browser.navigateBack.running',
    completed: 'agent.builtinCapability.browser.navigateBack.completed',
    failed: 'agent.builtinCapability.browser.navigateBack.failed',
    cancelled: 'agent.builtinCapability.browser.navigateBack.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.navigateBack.outcomeUnknown'
  },
  resize: {
    running: 'agent.builtinCapability.browser.resize.running',
    completed: 'agent.builtinCapability.browser.resize.completed',
    failed: 'agent.builtinCapability.browser.resize.failed',
    cancelled: 'agent.builtinCapability.browser.resize.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.resize.outcomeUnknown'
  },
  screenshot: {
    running: 'agent.builtinCapability.browser.screenshot.running',
    completed: 'agent.builtinCapability.browser.screenshot.completed',
    failed: 'agent.builtinCapability.browser.screenshot.failed',
    cancelled: 'agent.builtinCapability.browser.screenshot.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.screenshot.outcomeUnknown'
  },
  upload: {
    running: 'agent.builtinCapability.browser.upload.running',
    completed: 'agent.builtinCapability.browser.upload.completed',
    failed: 'agent.builtinCapability.browser.upload.failed',
    cancelled: 'agent.builtinCapability.browser.upload.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.upload.outcomeUnknown'
  },
  script: {
    running: 'agent.builtinCapability.browser.script.running',
    completed: 'agent.builtinCapability.browser.script.completed',
    failed: 'agent.builtinCapability.browser.script.failed',
    cancelled: 'agent.builtinCapability.browser.script.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.script.outcomeUnknown'
  },
  config: {
    running: 'agent.builtinCapability.browser.config.running',
    completed: 'agent.builtinCapability.browser.config.completed',
    failed: 'agent.builtinCapability.browser.config.failed',
    cancelled: 'agent.builtinCapability.browser.config.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.config.outcomeUnknown'
  },
  console: {
    running: 'agent.builtinCapability.browser.console.running',
    completed: 'agent.builtinCapability.browser.console.completed',
    failed: 'agent.builtinCapability.browser.console.failed',
    cancelled: 'agent.builtinCapability.browser.console.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.console.outcomeUnknown'
  },
  networkRead: {
    running: 'agent.builtinCapability.browser.networkRead.running',
    completed: 'agent.builtinCapability.browser.networkRead.completed',
    failed: 'agent.builtinCapability.browser.networkRead.failed',
    cancelled: 'agent.builtinCapability.browser.networkRead.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.networkRead.outcomeUnknown'
  },
  networkConfigure: {
    running: 'agent.builtinCapability.browser.networkConfigure.running',
    completed: 'agent.builtinCapability.browser.networkConfigure.completed',
    failed: 'agent.builtinCapability.browser.networkConfigure.failed',
    cancelled: 'agent.builtinCapability.browser.networkConfigure.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.networkConfigure.outcomeUnknown'
  },
  storageRead: {
    running: 'agent.builtinCapability.browser.storageRead.running',
    completed: 'agent.builtinCapability.browser.storageRead.completed',
    failed: 'agent.builtinCapability.browser.storageRead.failed',
    cancelled: 'agent.builtinCapability.browser.storageRead.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.storageRead.outcomeUnknown'
  },
  storageWrite: {
    running: 'agent.builtinCapability.browser.storageWrite.running',
    completed: 'agent.builtinCapability.browser.storageWrite.completed',
    failed: 'agent.builtinCapability.browser.storageWrite.failed',
    cancelled: 'agent.builtinCapability.browser.storageWrite.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.storageWrite.outcomeUnknown'
  },
  devtools: {
    running: 'agent.builtinCapability.browser.devtools.running',
    completed: 'agent.builtinCapability.browser.devtools.completed',
    failed: 'agent.builtinCapability.browser.devtools.failed',
    cancelled: 'agent.builtinCapability.browser.devtools.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.devtools.outcomeUnknown'
  },
  resume: {
    running: 'agent.builtinCapability.browser.resume.running',
    completed: 'agent.builtinCapability.browser.resume.completed',
    failed: 'agent.builtinCapability.browser.resume.failed',
    cancelled: 'agent.builtinCapability.browser.resume.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.resume.outcomeUnknown'
  },
  networkRules: {
    running: 'agent.builtinCapability.browser.networkRules.running',
    completed: 'agent.builtinCapability.browser.networkRules.completed',
    failed: 'agent.builtinCapability.browser.networkRules.failed',
    cancelled: 'agent.builtinCapability.browser.networkRules.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.networkRules.outcomeUnknown'
  },
  recordStart: {
    running: 'agent.builtinCapability.browser.recordStart.running',
    completed: 'agent.builtinCapability.browser.recordStart.completed',
    failed: 'agent.builtinCapability.browser.recordStart.failed',
    cancelled: 'agent.builtinCapability.browser.recordStart.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.recordStart.outcomeUnknown'
  },
  recordStop: {
    running: 'agent.builtinCapability.browser.recordStop.running',
    completed: 'agent.builtinCapability.browser.recordStop.completed',
    failed: 'agent.builtinCapability.browser.recordStop.failed',
    cancelled: 'agent.builtinCapability.browser.recordStop.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.recordStop.outcomeUnknown'
  },
  recordEdit: {
    running: 'agent.builtinCapability.browser.recordEdit.running',
    completed: 'agent.builtinCapability.browser.recordEdit.completed',
    failed: 'agent.builtinCapability.browser.recordEdit.failed',
    cancelled: 'agent.builtinCapability.browser.recordEdit.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.recordEdit.outcomeUnknown'
  },
  pdf: {
    running: 'agent.builtinCapability.browser.pdf.running',
    completed: 'agent.builtinCapability.browser.pdf.completed',
    failed: 'agent.builtinCapability.browser.pdf.failed',
    cancelled: 'agent.builtinCapability.browser.pdf.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.pdf.outcomeUnknown'
  },
  locator: {
    running: 'agent.builtinCapability.browser.locator.running',
    completed: 'agent.builtinCapability.browser.locator.completed',
    failed: 'agent.builtinCapability.browser.locator.failed',
    cancelled: 'agent.builtinCapability.browser.locator.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.locator.outcomeUnknown'
  },
  verify: {
    running: 'agent.builtinCapability.browser.verify.running',
    completed: 'agent.builtinCapability.browser.verify.completed',
    failed: 'agent.builtinCapability.browser.verify.failed',
    cancelled: 'agent.builtinCapability.browser.verify.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.verify.outcomeUnknown'
  }
} as const satisfies Record<BrowserToolFamily, Record<BrowserToolStatus, TranslationKey>>

const fallbackStatusKeys = {
  running: 'agent.builtinCapability.browser.fallback.running',
  completed: 'agent.builtinCapability.browser.fallback.completed',
  failed: 'agent.builtinCapability.browser.fallback.failed',
  cancelled: 'agent.builtinCapability.browser.fallback.cancelled',
  outcomeUnknown: 'agent.builtinCapability.browser.fallback.outcomeUnknown'
} as const satisfies Record<BrowserToolStatus, TranslationKey>

function safeProjectedStatus(result: AgentToolResult | undefined): string | null {
  const value = result?.result
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null
  const record = value as Record<string, unknown>
  if (
    record.schemaVersion !== 1 ||
    record.type !== 'builtin_capability_tool' ||
    record.contentOmitted !== true
  ) {
    return null
  }
  return typeof record.status === 'string' ? record.status : null
}

function safeProjectedArtifacts(result: AgentToolResult | undefined): BrowserArtifactReference[] {
  try {
    return parseBrowserArtifactToolProjection(result?.result).artifacts ?? []
  } catch {
    return []
  }
}

function getStatus(
  result: AgentToolResult | undefined,
  settledStatus: SettledToolStatus | undefined,
  cancelled: boolean
): BrowserToolStatus {
  const projectedStatus = safeProjectedStatus(result)
  if (projectedStatus === 'outcome_unknown') return 'outcomeUnknown'
  if (projectedStatus === 'cancelled') return 'cancelled'
  if (projectedStatus === 'failed' || result?.ok === false) return 'failed'
  if (projectedStatus === 'completed' || result) return 'completed'
  return settledStatus ?? (cancelled ? 'cancelled' : 'running')
}

function getStatusKey(toolId: string, status: BrowserToolStatus): TranslationKey {
  const family = Object.hasOwn(browserToolFamilyById, toolId)
    ? browserToolFamilyById[toolId as keyof typeof browserToolFamilyById]
    : null
  const toolKeys = family ? browserToolStatusKeys[family] : fallbackStatusKeys
  return toolKeys[status]
}

/** Product-safe activity for a Host-reviewed managed browser Tool. */
export function BuiltinCapabilityToolActivity({
  cancelled = false,
  displayReason,
  identity,
  result,
  settledStatus
}: BuiltinCapabilityToolActivityProps) {
  const { t } = useFrontendConfig()
  const reason = displayReason ? toSafeMcpDisplayText(displayReason, 512).trim() : ''
  const artifacts = safeProjectedArtifacts(result)
  const status = getStatus(result, settledStatus, cancelled)
  const Icon =
    status === 'outcomeUnknown'
      ? CircleAlert
      : status === 'failed'
        ? XCircle
        : status === 'completed'
          ? CheckCircle2
          : status === 'cancelled'
            ? Ban
            : LoaderCircle

  return (
    <AgentActivityDisclosure
      className="builtin-capability-tool-activity"
      hasDetails={Boolean(reason) || artifacts.length > 0}
      icon={Icon}
      isPending={status === 'running'}
      label={t(getStatusKey(identity.toolId, status))}
    >
      {reason ? (
        <div className="agent-activity__details">
          <span>{t('agent.builtinCapability.activity.reason')}</span>
          <p>{reason}</p>
        </div>
      ) : null}
      <BrowserArtifactCards artifacts={artifacts} />
    </AgentActivityDisclosure>
  )
}
