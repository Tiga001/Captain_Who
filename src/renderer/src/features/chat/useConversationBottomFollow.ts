// Chat scrolling: keeps the message list pinned to the newest content until the reader scrolls
// away, and reports the bottom state for the conversation's return-to-bottom control.
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react'

/** Distances within this many pixels of the bottom still count as resting at the bottom. */
export const CONVERSATION_BOTTOM_THRESHOLD_PX = 20

function getDistanceFromBottom(element: HTMLElement): number {
  return Math.max(0, element.scrollHeight - element.scrollTop - element.clientHeight)
}

export interface ConversationBottomFollowState {
  /** True while the visible scroll position rests within the bottom threshold. */
  isAtBottom: boolean
  /** Jumps to the newest content and resumes following it. */
  scrollToBottom: () => void
}

export function useConversationBottomFollow(
  scrollContainerRef: RefObject<HTMLDivElement | null>,
  conversationId: string,
  enabled: boolean
): ConversationBottomFollowState {
  const [isAtBottom, setIsAtBottom] = useState(true)
  /** Whether incoming content should keep the viewport glued to the bottom. */
  const followsBottomRef = useRef(true)
  const lastScrollTopRef = useRef(0)

  const syncBottomState = useCallback(() => {
    const element = scrollContainerRef.current
    if (!element) return
    const movedUp = element.scrollTop < lastScrollTopRef.current
    lastScrollTopRef.current = element.scrollTop
    const atBottom = getDistanceFromBottom(element) <= CONVERSATION_BOTTOM_THRESHOLD_PX
    if (atBottom) {
      followsBottomRef.current = true
    } else if (movedUp) {
      // Only an upward move means the reader left the newest content; bottom-side growth that
      // races a programmatic scroll must not stop the follow.
      followsBottomRef.current = false
    }
    setIsAtBottom(atBottom)
  }, [scrollContainerRef])

  const pinToBottom = useCallback(() => {
    const element = scrollContainerRef.current
    if (!element || !followsBottomRef.current) return
    if (element.scrollTop < lastScrollTopRef.current) {
      // The viewport moved up since the last observed position; growth raced a scroll the
      // reader already made, so treat it like that scroll instead of pulling them back.
      syncBottomState()
      return
    }
    element.scrollTop = element.scrollHeight
    lastScrollTopRef.current = element.scrollTop
  }, [scrollContainerRef, syncBottomState])

  const scrollToBottom = useCallback(() => {
    const element = scrollContainerRef.current
    if (!element) return
    followsBottomRef.current = true
    const reduceMotion = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
    if (reduceMotion) {
      element.scrollTop = element.scrollHeight
      return
    }
    element.scrollTo({ behavior: 'smooth', top: element.scrollHeight })
  }, [scrollContainerRef])

  useLayoutEffect(() => {
    if (!enabled) return undefined
    // A restored position or a fresh bottom landing can miss its scroll event on mount.
    const animationFrameId = window.requestAnimationFrame(() => {
      const element = scrollContainerRef.current
      if (!element) return
      lastScrollTopRef.current = element.scrollTop
      const atBottom = getDistanceFromBottom(element) <= CONVERSATION_BOTTOM_THRESHOLD_PX
      followsBottomRef.current = atBottom
      setIsAtBottom(atBottom)
    })
    return () => window.cancelAnimationFrame(animationFrameId)
  }, [conversationId, enabled, scrollContainerRef])

  useEffect(() => {
    if (!enabled) return undefined
    const element = scrollContainerRef.current
    if (!element) return undefined
    const handleScroll = () => syncBottomState()
    const handleResize = () => {
      // While following, a resized viewport keeps the newest content at the bottom.
      if (followsBottomRef.current) {
        pinToBottom()
        return
      }
      syncBottomState()
    }
    element.addEventListener('scroll', handleScroll, { passive: true })
    const resizeObserver =
      typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(handleResize)
    resizeObserver?.observe(element)
    return () => {
      element.removeEventListener('scroll', handleScroll)
      resizeObserver?.disconnect()
    }
  }, [conversationId, enabled, pinToBottom, scrollContainerRef, syncBottomState])

  useEffect(() => {
    if (!enabled) return undefined
    const element = scrollContainerRef.current
    if (!element) return undefined
    let animationFrameId: number | null = null
    const followNewContent = () => {
      if (animationFrameId !== null) return
      animationFrameId = window.requestAnimationFrame(() => {
        animationFrameId = null
        pinToBottom()
      })
    }
    // Streaming text and appended timeline entries mutate the subtree; the bottom stays glued
    // only while the reader has not scrolled away.
    const observer = new MutationObserver(followNewContent)
    observer.observe(element, { characterData: true, childList: true, subtree: true })
    return () => {
      observer.disconnect()
      if (animationFrameId !== null) window.cancelAnimationFrame(animationFrameId)
    }
  }, [conversationId, enabled, pinToBottom, scrollContainerRef])

  return { isAtBottom, scrollToBottom }
}
