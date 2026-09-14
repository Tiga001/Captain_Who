import { BookOpen } from 'lucide-react'
import { useEffect, useId, useRef, useState, type KeyboardEvent } from 'react'
import { CaptainWhoLineIcon } from '../../../components/icons/CaptainWhoLineIcon'
import type { TranslationKey } from '../../../config/frontendTranslations'
import { hostClient } from '../../../host/hostClient'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'

interface LeftSidebarHelpMenuProps {
  t: (key: TranslationKey) => string
  onOpen: () => void
}

/** Only mounted while the footer has no visible update action or progress. */
export function LeftSidebarHelpMenu({ t, onOpen }: LeftSidebarHelpMenuProps) {
  const [isOpen, setOpen] = useState(false)
  const [failed, setFailed] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const aboutRef = useRef<HTMLButtonElement>(null)
  const docsRef = useRef<HTMLButtonElement>(null)
  const initialItem = useRef(0)
  const mounted = useRef(false)
  const attempt = useRef(0)
  const menuId = useId()
  const errorId = useId()

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  useEffect(() => {
    if (isOpen) (initialItem.current === 0 ? aboutRef : docsRef).current?.focus()
  }, [isOpen])
  useDismissOnOutsidePointer(rootRef, isOpen, () => setOpen(false))

  const open = (last = false) => {
    ++attempt.current
    initialItem.current = last ? 1 : 0
    setFailed(false)
    onOpen()
    setOpen(true)
  }
  const close = () => {
    setOpen(false)
    triggerRef.current?.focus({ preventScroll: true })
  }
  const perform = async (action: 'showAbout' | 'openDocumentation') => {
    const currentAttempt = ++attempt.current
    close()
    setFailed(false)
    try {
      // No renderer-supplied URL or native panel options cross this boundary.
      await hostClient.app[action]()
    } catch {
      if (mounted.current && attempt.current === currentAttempt) setFailed(true)
    }
  }
  const handleMenuKey = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      close()
    } else if (event.key === 'Tab') {
      // Return to the trigger before native Tab traversal leaves the menu.
      close()
    } else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault()
      const next =
        event.key === 'Home'
          ? aboutRef
          : event.key === 'End'
            ? docsRef
            : document.activeElement === aboutRef.current
              ? docsRef
              : aboutRef
      next.current?.focus()
    }
  }

  return (
    <div
      className="left-sidebar__help"
      ref={rootRef}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false)
      }}
    >
      <button
        className="left-sidebar__help-button"
        type="button"
        ref={triggerRef}
        title={t('help.menu')}
        aria-label={t('help.menu')}
        aria-haspopup="menu"
        aria-expanded={isOpen}
        aria-controls={isOpen ? menuId : undefined}
        aria-describedby={failed ? errorId : undefined}
        onClick={() => (isOpen ? close() : open())}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault()
            open(event.key === 'ArrowUp')
          }
        }}
      >
        <svg
          className="left-sidebar__help-icon"
          viewBox="0 0 24 24"
          width="18"
          height="18"
          fill="none"
          stroke="currentColor"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
          focusable="false"
        >
          <circle cx="12" cy="12" r="9.25" strokeWidth="1.5" />
          <path
            d="M9.6 9.1C9.6 7.9 10.6 7 12 7s2.4.85 2.4 2.1c0 1.55-2.4 1.85-2.4 3.55"
            strokeWidth="1.8"
          />
          <circle cx="12" cy="16.25" r=".9" fill="currentColor" stroke="none" />
        </svg>
      </button>
      {isOpen && (
        <div
          className="left-sidebar__account-menu left-sidebar__help-menu"
          role="menu"
          id={menuId}
          aria-label={t('help.menu')}
          onKeyDown={handleMenuKey}
        >
          <button
            className="left-sidebar__account-menu-item"
            type="button"
            role="menuitem"
            tabIndex={-1}
            ref={aboutRef}
            onClick={() => void perform('showAbout')}
          >
            <CaptainWhoLineIcon />
            <span>{t('help.about')}</span>
          </button>
          <button
            className="left-sidebar__account-menu-item"
            type="button"
            role="menuitem"
            tabIndex={-1}
            ref={docsRef}
            onClick={() => void perform('openDocumentation')}
          >
            <BookOpen aria-hidden="true" />
            <span>{t('help.documentation')}</span>
          </button>
        </div>
      )}
      {failed && (
        <span className="left-sidebar__help-error" id={errorId} role="alert">
          {t('help.openFailed')}
        </span>
      )}
    </div>
  )
}
