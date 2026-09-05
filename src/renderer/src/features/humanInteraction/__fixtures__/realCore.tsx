// Only the Electron transport is adapted. The Preload bridge, Main handlers, Core, controller,
// settings and panel are production modules; no question/answer outcome is synthesized here.
import { createRoot } from 'react-dom/client'
import { createHumanInteractionIpcBridge } from '../../../../../preload/HumanInteractionIpcBridge'
import type { IpcRenderer } from 'electron'
import type { HostApi } from '@mycopilot/host-api'
import type { StorageChatConversationRecord } from '@mycopilot/protocol'
import '../../../styles/global.css'

declare global {
  interface Window {
    __humanInvoke: (channel: string, input: unknown) => Promise<unknown>
    __loadHumanConversation: (id: string) => Promise<StorageChatConversationRecord | null>
  }
}
const listeners = new Map<(...args: never[]) => unknown, EventListener>()
const ipc = {
  invoke: (channel: string, input: unknown) => window.__humanInvoke(channel, input),
  on: (channel: string, callback: (...args: never[]) => unknown) => {
    const listener: EventListener = (event) =>
      callback({} as never, (event as CustomEvent).detail as never)
    listeners.set(callback, listener)
    window.addEventListener(channel, listener)
  },
  removeListener: (channel: string, callback: (...args: never[]) => unknown) => {
    const listener = listeners.get(callback)
    if (listener) window.removeEventListener(channel, listener)
    listeners.delete(callback)
  }
} as unknown as IpcRenderer
window.mycopilot = {
  host: {
    humanInteraction: createHumanInteractionIpcBridge(ipc),
    app: { setNativeThemeSource: async () => undefined },
    storage: { loadConversation: window.__loadHumanConversation }
  } as unknown as HostApi
}
const { FrontendConfigProvider } = await import('../../../config/FrontendConfigProvider')
const { HumanInteractionSettingsSection } =
  await import('../../settings/pages/HumanInteractionSettingsSection')
const { useHumanInteraction } = await import('../useHumanInteraction')
const { HumanInteractionPanel } = await import('../HumanInteractionPanel')
const { HumanInteractionTimelineEntry } = await import('../HumanInteractionTimelineEntry')
const { HumanInteractionAnswerContent } = await import('../HumanInteractionAnswerContent')

function Workspace() {
  const conversationId = new URLSearchParams(location.search).get('conversation')!
  const state = useHumanInteraction({ conversationId, hasApproval: false })
  const request = state.activeBatch
  return (
    <main style={{ maxWidth: 720, margin: '20px auto' }}>
      <HumanInteractionSettingsSection />
      <div data-testid="entries">
        {state.openRequests.map((batch) => (
          <HumanInteractionTimelineEntry
            key={batch.requestId}
            request={batch}
            interaction={state}
          />
        ))}
      </div>
      {request && (
        <HumanInteractionPanel
          request={request}
          pageIndex={state.activeDraft.pageIndex}
          answers={state.activeDraft.answers}
          canSubmit={state.canSubmit}
          isSubmitting={state.isSubmitting}
          isDraftLocked={state.isDraftLocked}
          error={state.error}
          onPageChange={(index) => state.setPage(request.requestId, index)}
          onAnswerChange={(answer) => state.setAnswer(request.requestId, answer)}
          onSubmit={() => void state.submit(request.requestId)}
          onIgnore={() => void state.ignore(request.requestId)}
          onMinimize={() => state.minimize(request.requestId)}
        />
      )}
      {state.historyResponses.map(({ display }) => (
        <HumanInteractionAnswerContent key={display.responseId} display={display} />
      ))}
    </main>
  )
}
createRoot(document.getElementById('root')!).render(
  <FrontendConfigProvider>
    <Workspace />
  </FrontendConfigProvider>
)
