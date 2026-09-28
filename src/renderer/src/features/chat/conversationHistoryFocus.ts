/** Release search focus before programmatic conversation navigation removes the input. */
export function blurConversationHistorySearch() {
  if (typeof document === 'undefined') return
  const active = document.activeElement
  if (
    active instanceof HTMLInputElement &&
    active.matches('.conversation-history-tools__search input')
  )
    active.blur()
}
