import {
  ChevronRight,
  LayoutDashboard,
  ArrowDownToLine,
  ArrowUpFromLine,
  MessageCircle,
  Network
} from 'lucide-react'
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { AnchoredPopover } from '../../components/overlay/AnchoredPopover'
import { Tooltip } from '../../components/overlay/Tooltip'
import type { Translate } from '../../config/translationFormat'
import type { WorkflowNeighborNode } from '../../features/workflows/workflowNeighborNodes'
import { useDismissOnOutsidePointer } from '../../hooks/useDismissOnOutsidePointer'
import './MainPanelWorkflowMenu.css'

export interface MainPanelWorkflow {
  name: string
  color: string
  upstream: WorkflowNeighborNode[]
  downstream: WorkflowNeighborNode[]
  onOpen: () => void
  onOpenConversation: (conversationId: string) => void
}

type Submenu = 'upstream' | 'downstream'

/** Submenu width plus its gap and viewport margin. */
const SUBMENU_SPACE = 240

export function MainPanelWorkflowMenu({
  onOpenChange,
  open,
  t,
  workflow
}: {
  onOpenChange: (open: boolean) => void
  open: boolean
  t: Translate
  workflow: MainPanelWorkflow
}) {
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  const [submenu, setSubmenu] = useState<Submenu | null>(null)
  const [submenuSide, setSubmenuSide] = useState<'left' | 'right'>('right')
  useLayoutEffect(() => {
    const menu = menuRef.current
    if (!submenu || !menu) return
    const rect = menu.getBoundingClientRect()
    setSubmenuSide(
      window.innerWidth - rect.right >= SUBMENU_SPACE || rect.left < window.innerWidth - rect.right
        ? 'right'
        : 'left'
    )
  }, [submenu])
  const close = () => {
    setSubmenu(null)
    onOpenChange(false)
  }
  useDismissOnOutsidePointer(rootRef, open, close, (target) =>
    Boolean(popoverRef.current?.contains(target))
  )

  const submenuItem = (kind: Submenu, icon: ReactNode, label: string) => (
    <button
      className="main-panel__workflow-menu-item"
      type="button"
      role="menuitem"
      aria-haspopup="menu"
      aria-expanded={submenu === kind}
      data-open={submenu === kind || undefined}
      onMouseEnter={() => setSubmenu(kind)}
      onClick={() => setSubmenu(kind)}
    >
      {icon}
      <span>{label}</span>
      <ChevronRight aria-hidden="true" />
    </button>
  )
  const nodes = submenu === 'upstream' ? workflow.upstream : workflow.downstream

  return (
    <div className="main-panel__workflow" ref={rootRef}>
      <Tooltip content={workflow.name} disabled={open} preferredPlacement="bottom">
        <button
          className="main-panel__workflow-button"
          type="button"
          aria-expanded={open}
          aria-haspopup="menu"
          aria-label={workflow.name}
          data-open={open || undefined}
          style={{ color: workflow.color }}
          onClick={() => (open ? close() : onOpenChange(true))}
          ref={triggerRef}
        >
          <Network aria-hidden="true" />
        </button>
      </Tooltip>
      {open ? (
        <AnchoredPopover
          align="start"
          anchorRef={triggerRef}
          className="main-panel__workflow-popover"
          enabled
          onClose={close}
          popoverRef={popoverRef}
        >
          <div
            className="main-panel__workflow-menu"
            role="menu"
            aria-label={workflow.name}
            ref={menuRef}
          >
            <button
              className="main-panel__workflow-menu-item"
              type="button"
              role="menuitem"
              onMouseEnter={() => setSubmenu(null)}
              onClick={() => {
                close()
                workflow.onOpen()
              }}
            >
              <LayoutDashboard aria-hidden="true" />
              <span>{t('workflows.openBoard')}</span>
            </button>
            {submenuItem(
              'upstream',
              <ArrowUpFromLine aria-hidden="true" />,
              t('workflows.upstreamNodes')
            )}
            {submenuItem(
              'downstream',
              <ArrowDownToLine aria-hidden="true" />,
              t('workflows.downstreamNodes')
            )}
            {submenu ? (
              <div
                className="main-panel__workflow-submenu"
                data-kind={submenu}
                data-side={submenuSide}
                role="menu"
                aria-label={t(
                  submenu === 'upstream' ? 'workflows.upstreamNodes' : 'workflows.downstreamNodes'
                )}
              >
                {nodes.length ? (
                  nodes.map((node) => (
                    <button
                      className="main-panel__workflow-menu-item"
                      type="button"
                      role="menuitem"
                      key={node.nodeId}
                      onClick={() => {
                        close()
                        workflow.onOpenConversation(node.conversationId)
                      }}
                    >
                      <MessageCircle aria-hidden="true" />
                      <span>{node.name}</span>
                    </button>
                  ))
                ) : (
                  <div className="main-panel__workflow-menu-empty">{t('workflows.noNodes')}</div>
                )}
              </div>
            ) : null}
          </div>
        </AnchoredPopover>
      ) : null}
    </div>
  )
}
