// Renderer UI.
import {
  Archive,
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  Clock3,
  Folder,
  FolderOpen,
  Mail,
  MoreHorizontal,
  NotebookText,
  PencilLine,
  Pin,
  Settings,
  SquarePen,
  X,
} from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { useFrontendConfig } from "../../config/FrontendConfigProvider";
import type { AppProject } from "../../config/projectConfig";
import { ConfirmationDialog } from "../dialog/ConfirmationDialog";
import { TextInputDialog } from "../dialog/TextInputDialog";
import type { ChatConversation } from "../../features/chat/chatTypes";
import {
  getProfileDisplayName,
  getProfileHandle,
  getProfileInitials,
} from "../../features/profile/profileUtils";
import type {
  SidebarConversationSort,
  SidebarProjectSort,
  UiPreferencesSnapshot,
} from "../../features/storage/storageClient";
import { useDismissOnOutsidePointer } from "../../hooks/useDismissOnOutsidePointer";
import "./LeftSidebar.css";

type SidebarSectionScope = "projects" | "conversations";
type SidebarSectionSubmenu = "organize" | "sort";
type BulkArchiveScope = "projects" | "root";
type ProjectDragPosition = "before" | "after";
type ConversationListStage = "collapsed" | "preview" | "expanded";

const COLLAPSED_CONVERSATION_COUNT = 5;
const PREVIEW_CONVERSATION_COUNT = 10;
const PREVIEW_CONVERSATION_THRESHOLD = 12;
const ROOT_CONVERSATION_LIST_KEY = "root";

interface ProjectDragTarget {
  projectId: string;
  position: ProjectDragPosition;
}

interface ProjectPointerDragState {
  hasMoved: boolean;
  pointerId: number;
  projectId: string;
  sectionProjectIds: string[];
  startY: number;
}

interface LeftSidebarProps {
  activeConversationId: string | null;
  conversations: ChatConversation[];
  onArchiveAllProjectConversations: () => void;
  onArchiveAllRootConversations: () => void;
  onArchiveConversation: (conversationId: string) => void;
  onArchiveProjectConversations: (projectId: string) => void;
  onMarkConversationUnread: (conversationId: string) => void;
  onNewConversation: (projectId?: string | null) => void;
  onOpenSettings: () => void;
  onRemoveProject: (projectId: string) => void;
  onRenameConversation: (conversationId: string, title: string) => void;
  onRenameProject: (projectId: string, name: string) => void;
  onSelectConversation: (conversationId: string) => void;
  onShowProjectInFolder: (projectId: string) => void;
  onTogglePinConversation: (conversationId: string) => void;
  onTogglePinProject: (projectId: string) => void;
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void;
  projects: AppProject[];
  uiPreferences: UiPreferencesSnapshot;
}

function formatTemplate(template: string, values: Record<string, number | string>) {
  return Object.entries(values).reduce(
    (text, [key, value]) => text.split(`{${key}}`).join(String(value)),
    template,
  );
}

function formatConversationAge(updatedAt: number, now: number, language: string, justNow: string) {
  const elapsed = Math.max(0, now - updatedAt);
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return justNow;

  if (minutes < 60) {
    return language === "zh-CN" ? `${minutes} 分` : `${minutes}m`;
  }

  const hours = Math.floor(elapsed / 3_600_000);
  if (hours < 24) {
    return language === "zh-CN" ? `${hours} 小时` : `${hours}h`;
  }

  const days = Math.floor(elapsed / 86_400_000);
  if (days < 7) {
    return language === "zh-CN" ? `${days} 天` : `${days}d`;
  }

  const weeks = Math.floor(days / 7);
  if (weeks < 5) {
    return language === "zh-CN" ? `${weeks} 周` : `${weeks}w`;
  }

  const months = Math.floor(days / 30);
  if (months < 12) {
    return language === "zh-CN" ? `${months} 个月` : `${months}mo`;
  }

  const years = Math.floor(days / 365);
  return language === "zh-CN" ? `${Math.max(1, years)} 年` : `${Math.max(1, years)}y`;
}

function sortConversations(conversations: ChatConversation[], sort: SidebarConversationSort) {
  return [...conversations].sort((a, b) => {
    const aPinned = a.pinnedAt ?? 0;
    const bPinned = b.pinnedAt ?? 0;

    if (aPinned || bPinned) {
      if (aPinned && bPinned) return bPinned - aPinned;
      return aPinned ? -1 : 1;
    }

    if (sort === "created") return b.createdAt - a.createdAt;
    return b.updatedAt - a.updatedAt;
  });
}

function sortPinnedProjects(projects: AppProject[]) {
  return [...projects]
    .filter((project) => project.pinnedAt)
    .sort((a, b) => (b.pinnedAt ?? 0) - (a.pinnedAt ?? 0));
}

function sortProjectsByManualOrder(projects: AppProject[], projectOrder: string[]) {
  const orderIndex = new Map(projectOrder.map((projectId, index) => [projectId, index]));

  return [...projects].sort((a, b) => {
    const aIndex = orderIndex.get(a.id);
    const bIndex = orderIndex.get(b.id);

    if (aIndex !== undefined || bIndex !== undefined) {
      if (aIndex !== undefined && bIndex !== undefined) return aIndex - bIndex;
      return aIndex !== undefined ? -1 : 1;
    }

    return a.createdAt - b.createdAt;
  });
}

function sortRegularProjects(
  projects: AppProject[],
  conversationsByProjectId: Record<string, ChatConversation[]>,
  sort: SidebarProjectSort,
  projectOrder: string[],
) {
  const regularProjects = projects.filter((project) => !project.pinnedAt);

  if (sort === "manual") {
    return sortProjectsByManualOrder(regularProjects, projectOrder);
  }

  return [...regularProjects].sort((a, b) => {
    if (sort === "recent") {
      const aRecent = latestProjectConversationUpdatedAt(conversationsByProjectId[a.id] ?? []);
      const bRecent = latestProjectConversationUpdatedAt(conversationsByProjectId[b.id] ?? []);
      if (aRecent !== bRecent) return bRecent - aRecent;
    }

    return a.createdAt - b.createdAt;
  });
}

function latestProjectConversationUpdatedAt(conversations: ChatConversation[]) {
  return conversations.reduce((latest, conversation) => Math.max(latest, conversation.updatedAt), 0);
}

function ConversationPinIcon({ filled }: { filled: boolean }) {
  return (
    <svg
      className="left-sidebar__conversation-pin-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M12 17v5" />
      <path
        d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"
        fill={filled ? "currentColor" : "none"}
      />
    </svg>
  );
}

function ConversationRow({
  activeConversationId,
  archiveLabel,
  conversation,
  justNow,
  language,
  markUnreadLabel,
  now,
  onArchiveConversation,
  onMarkConversationUnread,
  onRenameConversation,
  onSelectConversation,
  onTogglePinConversation,
  pinLabel,
  processingLabel,
  renameLabel,
  unreadLabel,
  unpinLabel,
  waitingApprovalLabel,
  nested = false,
}: {
  activeConversationId: string | null;
  archiveLabel: string;
  conversation: ChatConversation;
  justNow: string;
  language: string;
  markUnreadLabel: string;
  now: number;
  onArchiveConversation: (conversationId: string) => void;
  onMarkConversationUnread: (conversationId: string) => void;
  onRenameConversation: (conversation: ChatConversation) => void;
  onSelectConversation: (conversationId: string) => void;
  onTogglePinConversation: (conversationId: string) => void;
  pinLabel: string;
  processingLabel: string;
  renameLabel: string;
  unreadLabel: string;
  unpinLabel: string;
  waitingApprovalLabel: string;
  nested?: boolean;
}) {
  const conversationMenuRef = useRef<HTMLDivElement>(null);
  const [menuPosition, setMenuPosition] = useState<{ x: number; y: number } | null>(null);
  const isPinned = Boolean(conversation.pinnedAt);
  const isPending = conversation.messages.some(
    (message) => message.role === "assistant" && message.status === "pending",
  );
  const isWaitingForApproval = conversation.messages.some(
    (message) => message.role === "assistant" && message.agentRun?.status === "waiting_for_approval",
  );
  const showWaitingApprovalBadge = isWaitingForApproval && conversation.id !== activeConversationId;
  const isUnread = Boolean(conversation.unreadAt && conversation.id !== activeConversationId && !isPending);
  const canMarkUnread = conversation.id !== activeConversationId && !isPending && !conversation.unreadAt;
  const isConversationMenuOpen = Boolean(menuPosition);

  const closeConversationMenu = () => setMenuPosition(null);
  useDismissOnOutsidePointer(conversationMenuRef, Boolean(menuPosition), closeConversationMenu);

  return (
    <div
      className={`left-sidebar__conversation-row${nested ? " left-sidebar__conversation-row--nested" : ""}`}
      data-active={conversation.id === activeConversationId || undefined}
      data-awaiting-approval={showWaitingApprovalBadge || undefined}
      data-menu-open={isConversationMenuOpen || undefined}
      data-pending={isPending || undefined}
      onContextMenu={(event) => {
        event.preventDefault();
        setMenuPosition({
          x: Math.max(12, Math.min(event.clientX, window.innerWidth - 238)),
          y: Math.max(12, Math.min(event.clientY, window.innerHeight - 190)),
        });
      }}
    >
      <button
        className="left-sidebar__conversation-main"
        type="button"
        onClick={() => onSelectConversation(conversation.id)}
      >
        <span className="left-sidebar__conversation-name">{conversation.title}</span>
      </button>

      <span className="left-sidebar__conversation-age">
        {showWaitingApprovalBadge ? (
          <>
            <span className="left-sidebar__conversation-approval-badge">{waitingApprovalLabel}</span>
            <span className="left-sidebar__conversation-spinner" aria-label={processingLabel} />
          </>
        ) : isPending ? (
          <span className="left-sidebar__conversation-spinner" aria-label={processingLabel} />
        ) : isUnread ? (
          <span className="left-sidebar__conversation-unread-dot" aria-label={unreadLabel} />
        ) : (
          formatConversationAge(conversation.updatedAt, now, language, justNow)
        )}
      </span>

      <div className="left-sidebar__conversation-item-actions" aria-label={archiveLabel}>
        <button
          className="left-sidebar__conversation-item-action"
          type="button"
          data-pinned={isPinned || undefined}
          data-tooltip={isPinned ? unpinLabel : pinLabel}
          aria-label={isPinned ? unpinLabel : pinLabel}
          disabled={isConversationMenuOpen}
          onClick={() => onTogglePinConversation(conversation.id)}
        >
          <ConversationPinIcon filled={isPinned} />
        </button>
        <button
          className="left-sidebar__conversation-item-action"
          type="button"
          data-tooltip={archiveLabel}
          aria-label={archiveLabel}
          disabled={isConversationMenuOpen}
          onClick={() => onArchiveConversation(conversation.id)}
        >
          <Archive aria-hidden="true" />
        </button>
      </div>

      {menuPosition && (
        <div
          className="left-sidebar__conversation-menu"
          role="menu"
          ref={conversationMenuRef}
          style={{ left: menuPosition.x, top: menuPosition.y }}
        >
          <button
            className="left-sidebar__project-menu-item"
            type="button"
            role="menuitem"
            onClick={() => {
              onTogglePinConversation(conversation.id);
              closeConversationMenu();
            }}
          >
            <Pin aria-hidden="true" />
            <span>{isPinned ? unpinLabel : pinLabel}</span>
          </button>
          <button
            className="left-sidebar__project-menu-item"
            type="button"
            role="menuitem"
            onClick={() => {
              onRenameConversation(conversation);
              closeConversationMenu();
            }}
          >
            <PencilLine aria-hidden="true" />
            <span>{renameLabel}</span>
          </button>
          <button
            className="left-sidebar__project-menu-item"
            type="button"
            role="menuitem"
            onClick={() => {
              onArchiveConversation(conversation.id);
              closeConversationMenu();
            }}
          >
            <Archive aria-hidden="true" />
            <span>{archiveLabel}</span>
          </button>
          <button
            className="left-sidebar__project-menu-item"
            type="button"
            role="menuitem"
            disabled={!canMarkUnread}
            onClick={() => {
              if (!canMarkUnread) return;

              onMarkConversationUnread(conversation.id);
              closeConversationMenu();
            }}
          >
            <Mail aria-hidden="true" />
            <span>{markUnreadLabel}</span>
          </button>
        </div>
      )}
    </div>
  );
}

export function LeftSidebar({
  activeConversationId,
  conversations,
  onArchiveAllProjectConversations,
  onArchiveAllRootConversations,
  onArchiveConversation,
  onArchiveProjectConversations,
  onMarkConversationUnread,
  onNewConversation,
  onOpenSettings,
  onRemoveProject,
  onRenameConversation,
  onRenameProject,
  onSelectConversation,
  onShowProjectInFolder,
  onTogglePinConversation,
  onTogglePinProject,
  onUiPreferencesChange,
  projects,
  uiPreferences,
}: LeftSidebarProps) {
  const { language, t } = useFrontendConfig();
  const [areProjectsOpen, setAreProjectsOpen] = useState(true);
  const [areConversationsOpen, setAreConversationsOpen] = useState(true);
  const [openProjectIds, setOpenProjectIds] = useState<Set<string>>(new Set());
  const [openProjectMenuId, setOpenProjectMenuId] = useState<string | null>(null);
  const [openSectionMenu, setOpenSectionMenu] = useState<SidebarSectionScope | null>(null);
  const [openSectionSubmenu, setOpenSectionSubmenu] = useState<SidebarSectionSubmenu | null>(null);
  const [renamingProject, setRenamingProject] = useState<AppProject | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [renamingConversation, setRenamingConversation] = useState<ChatConversation | null>(null);
  const [conversationRenameValue, setConversationRenameValue] = useState("");
  const [pendingBulkArchiveScope, setPendingBulkArchiveScope] = useState<BulkArchiveScope | null>(null);
  const [pendingArchiveProject, setPendingArchiveProject] = useState<AppProject | null>(null);
  const [pendingRemoveProject, setPendingRemoveProject] = useState<AppProject | null>(null);
  const [isAccountMenuOpen, setAccountMenuOpen] = useState(false);
  const [conversationListStages, setConversationListStages] = useState<Record<string, ConversationListStage>>({});
  const [draggingProjectId, setDraggingProjectId] = useState<string | null>(null);
  const [projectDragPreviewOrder, setProjectDragPreviewOrderState] = useState<string[] | null>(null);
  const [now, setNow] = useState(Date.now());
  const projectMenuRef = useRef<HTMLDivElement>(null);
  const sectionMenuRef = useRef<HTMLDivElement>(null);
  const accountMenuRef = useRef<HTMLDivElement>(null);
  const projectRowRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const projectAnimationRectsRef = useRef<Map<string, DOMRect> | null>(null);
  const projectPointerDragRef = useRef<ProjectPointerDragState | null>(null);
  const projectDragPreviewOrderRef = useRef<string[] | null>(null);
  const suppressProjectClickRef = useRef<string | null>(null);
  const projectIds = new Set(projects.map((project) => project.id));
  const visibleConversations = conversations.filter((conversation) => !conversation.archivedAt);
  const conversationsByProjectId = projects.reduce<Record<string, ChatConversation[]>>((accumulator, project) => {
    accumulator[project.id] = sortConversations(
      visibleConversations.filter((conversation) => conversation.projectId === project.id),
      uiPreferences.sidebarConversationSort,
    );
    return accumulator;
  }, {});
  const pinnedProjects = sortPinnedProjects(projects);
  const regularProjects = sortRegularProjects(
    projects,
    conversationsByProjectId,
    uiPreferences.sidebarProjectSort,
    uiPreferences.sidebarProjectOrder,
  );
  const displayRegularProjects = projectDragPreviewOrder
    ? sortProjectsByManualOrder(regularProjects, projectDragPreviewOrder)
    : regularProjects;
  const pinnedRootConversations = sortConversations(
    visibleConversations.filter(
      (conversation) =>
        conversation.pinnedAt && (!conversation.projectId || !projectIds.has(conversation.projectId)),
    ),
    uiPreferences.sidebarConversationSort,
  );
  const rootConversations = sortConversations(
    visibleConversations.filter(
      (conversation) =>
        !conversation.pinnedAt && (!conversation.projectId || !projectIds.has(conversation.projectId)),
    ),
    uiPreferences.sidebarConversationSort,
  );
  const activeConversation = visibleConversations.find((conversation) => conversation.id === activeConversationId);
  const profileDisplayName = getProfileDisplayName(uiPreferences, language);
  const profileHandle = getProfileHandle(uiPreferences);
  const profileInitials = getProfileInitials(profileDisplayName);
  const hasPinnedItems = pinnedProjects.length > 0 || pinnedRootConversations.length > 0;
  const projectArchiveAllCount = visibleConversations.filter(
    (conversation) => conversation.projectId && projectIds.has(conversation.projectId),
  ).length;
  const rootArchiveAllCount = visibleConversations.filter(
    (conversation) => !conversation.projectId || !projectIds.has(conversation.projectId),
  ).length;
  const bulkArchiveCount =
    pendingBulkArchiveScope === "projects" ? projectArchiveAllCount : rootArchiveAllCount;

  const closeProjectMenu = () => {
    setOpenProjectMenuId(null);
  };

  useDismissOnOutsidePointer(projectMenuRef, Boolean(openProjectMenuId), closeProjectMenu);
  useDismissOnOutsidePointer(sectionMenuRef, Boolean(openSectionMenu), () => {
    setOpenSectionMenu(null);
    setOpenSectionSubmenu(null);
  });
  useDismissOnOutsidePointer(accountMenuRef, isAccountMenuOpen, () => setAccountMenuOpen(false));

  useEffect(() => {
    const intervalId = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(intervalId);
  }, []);

  useEffect(() => {
    if (!activeConversation?.projectId) return;

    setOpenProjectIds((currentIds) => {
      if (currentIds.has(activeConversation.projectId!)) return currentIds;
      const nextIds = new Set(currentIds);
      nextIds.add(activeConversation.projectId!);
      return nextIds;
    });
  }, [activeConversation?.projectId]);

  useLayoutEffect(() => {
    const previousRects = projectAnimationRectsRef.current;
    if (!previousRects) return;

    projectAnimationRectsRef.current = null;

    for (const project of displayRegularProjects) {
      const element = projectRowRefs.current.get(project.id);
      const previousRect = previousRects.get(project.id);
      if (!element || !previousRect) continue;

      const nextRect = element.getBoundingClientRect();
      const deltaY = previousRect.top - nextRect.top;
      if (Math.abs(deltaY) < 1) continue;

      element.animate(
        [
          { transform: `translateY(${deltaY}px)` },
          { transform: "translateY(0)" },
        ],
        {
          duration: 150,
          easing: "cubic-bezier(0.2, 0, 0, 1)",
        },
      );
    }
  }, [displayRegularProjects]);

  const toggleProject = (projectId: string) => {
    setOpenProjectIds((currentIds) => {
      const nextIds = new Set(currentIds);
      if (nextIds.has(projectId)) {
        nextIds.delete(projectId);
      } else {
        nextIds.add(projectId);
      }
      return nextIds;
    });
  };

  const startRenamingProject = (project: AppProject) => {
    setRenameValue(project.name);
    setRenamingProject(project);
    closeProjectMenu();
  };

  const confirmRenameProject = () => {
    if (!renamingProject) return;

    const normalizedName = renameValue.trim();
    if (!normalizedName) return;

    onRenameProject(renamingProject.id, normalizedName);
    setRenamingProject(null);
    setRenameValue("");
  };

  const startRenamingConversation = (conversation: ChatConversation) => {
    setConversationRenameValue(conversation.title);
    setRenamingConversation(conversation);
  };

  const confirmRenameConversation = () => {
    if (!renamingConversation) return;

    const normalizedTitle = conversationRenameValue.trim();
    if (!normalizedTitle) return;

    onRenameConversation(renamingConversation.id, normalizedTitle);
    setRenamingConversation(null);
    setConversationRenameValue("");
  };

  const closeSectionMenu = () => {
    setOpenSectionMenu(null);
    setOpenSectionSubmenu(null);
  };

  const openSectionActions = (scope: SidebarSectionScope) => {
    setOpenSectionMenu((currentScope) => (currentScope === scope ? null : scope));
    setOpenSectionSubmenu(null);
    closeProjectMenu();
  };

  const setConversationSort = (sort: SidebarConversationSort) => {
    onUiPreferencesChange({ sidebarConversationSort: sort });
    closeSectionMenu();
  };

  const setProjectSort = (sort: SidebarProjectSort) => {
    onUiPreferencesChange({ sidebarProjectSort: sort });
    closeSectionMenu();
  };

  const buildProjectOrderWithSection = (sectionProjectIds: string[], nextSectionProjectIds: string[]) => {
    const allProjectIds = projects.map((project) => project.id);
    const sectionIdSet = new Set(sectionProjectIds);
    const knownIdSet = new Set(allProjectIds);
    const currentOrder = [
      ...uiPreferences.sidebarProjectOrder.filter((projectId) => knownIdSet.has(projectId)),
      ...allProjectIds.filter((projectId) => !uiPreferences.sidebarProjectOrder.includes(projectId)),
    ];
    const preservedIds = currentOrder.filter((projectId) => !sectionIdSet.has(projectId));

    return [...preservedIds, ...nextSectionProjectIds];
  };

  const setProjectDragPreviewOrder = (order: string[] | null) => {
    projectDragPreviewOrderRef.current = order;
    setProjectDragPreviewOrderState(order);
  };

  const captureProjectRowRects = (projectIds: string[]) => {
    const rects = new Map<string, DOMRect>();

    for (const projectId of projectIds) {
      const element = projectRowRefs.current.get(projectId);
      if (element) rects.set(projectId, element.getBoundingClientRect());
    }

    projectAnimationRectsRef.current = rects;
  };

  const resetProjectPointerDrag = () => {
    projectPointerDragRef.current = null;
    setDraggingProjectId(null);
    setProjectDragPreviewOrder(null);
  };

  const areProjectOrdersEqual = (a: string[], b: string[]) =>
    a.length === b.length && a.every((projectId, index) => projectId === b[index]);

  const buildMovedProjectOrder = (
    draggedProjectId: string,
    targetProjectId: string,
    position: ProjectDragPosition,
    sectionProjectIds: string[],
  ) => {
    if (draggedProjectId === targetProjectId) return sectionProjectIds;
    if (!sectionProjectIds.includes(draggedProjectId) || !sectionProjectIds.includes(targetProjectId)) {
      return sectionProjectIds;
    }

    const nextSectionProjectIds = sectionProjectIds.filter((projectId) => projectId !== draggedProjectId);
    const targetIndex = nextSectionProjectIds.indexOf(targetProjectId);
    nextSectionProjectIds.splice(position === "after" ? targetIndex + 1 : targetIndex, 0, draggedProjectId);

    return nextSectionProjectIds;
  };

  const getProjectDragTargetFromPoint = (
    clientX: number,
    clientY: number,
    sectionProjectIds: string[],
  ): ProjectDragTarget | null => {
    const targetElement = document.elementFromPoint(clientX, clientY);
    const projectElement = targetElement?.closest("[data-project-id]");

    if (!(projectElement instanceof HTMLElement)) return null;

    const projectId = projectElement.dataset.projectId;
    if (!projectId || !sectionProjectIds.includes(projectId)) return null;

    const rect = projectElement.getBoundingClientRect();
    return {
      projectId,
      position: clientY > rect.top + rect.height / 2 ? "after" : "before",
    };
  };

  const handleProjectPointerDown = (
    event: ReactPointerEvent<HTMLButtonElement>,
    project: AppProject,
    sectionProjects: AppProject[],
    isProjectOpen: boolean,
  ) => {
    if (event.button !== 0 || isProjectOpen || sectionProjects.length <= 1) return;
    if (
      (event.target as HTMLElement).closest(
        ".left-sidebar__project-icon, .left-sidebar__project-inline-chevron",
      )
    ) {
      return;
    }

    closeProjectMenu();
    closeSectionMenu();
    projectPointerDragRef.current = {
      hasMoved: false,
      pointerId: event.pointerId,
      projectId: project.id,
      sectionProjectIds: sectionProjects.map((sectionProject) => sectionProject.id),
      startY: event.clientY,
    };
    setProjectDragPreviewOrder(null);
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const handleProjectPointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const dragState = projectPointerDragRef.current;
    if (!dragState || dragState.pointerId !== event.pointerId) return;

    if (!dragState.hasMoved && Math.abs(event.clientY - dragState.startY) < 5) return;

    dragState.hasMoved = true;
    suppressProjectClickRef.current = dragState.projectId;
    setDraggingProjectId(dragState.projectId);

    const currentPreviewOrder = projectDragPreviewOrderRef.current ?? dragState.sectionProjectIds;
    const nextTarget = getProjectDragTargetFromPoint(
      event.clientX,
      event.clientY,
      currentPreviewOrder,
    );

    if (nextTarget && nextTarget.projectId !== dragState.projectId) {
      const nextPreviewOrder = buildMovedProjectOrder(
        dragState.projectId,
        nextTarget.projectId,
        nextTarget.position,
        currentPreviewOrder,
      );

      if (!areProjectOrdersEqual(currentPreviewOrder, nextPreviewOrder)) {
        captureProjectRowRects(currentPreviewOrder);
        setProjectDragPreviewOrder(nextPreviewOrder);
      }
    }

    event.preventDefault();
  };

  const handleProjectPointerUp = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const dragState = projectPointerDragRef.current;
    if (!dragState || dragState.pointerId !== event.pointerId) return;

    const finalPreviewOrder = projectDragPreviewOrderRef.current;
    if (
      dragState.hasMoved &&
      finalPreviewOrder &&
      !areProjectOrdersEqual(dragState.sectionProjectIds, finalPreviewOrder)
    ) {
      onUiPreferencesChange({
        sidebarProjectSort: "manual",
        sidebarProjectOrder: buildProjectOrderWithSection(dragState.sectionProjectIds, finalPreviewOrder),
      });
    }

    resetProjectPointerDrag();
    window.setTimeout(() => {
      suppressProjectClickRef.current = null;
    }, 0);
  };

  const handleProjectPointerCancel = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const dragState = projectPointerDragRef.current;
    if (!dragState || dragState.pointerId !== event.pointerId) return;

    resetProjectPointerDrag();
    window.setTimeout(() => {
      suppressProjectClickRef.current = null;
    }, 0);
  };

  const isSectionFirst = (scope: SidebarSectionScope) =>
    scope === "projects"
      ? uiPreferences.sidebarSectionOrder === "projects_first"
      : uiPreferences.sidebarSectionOrder === "conversations_first";

  const toggleSectionOrder = (scope: SidebarSectionScope) => {
    const nextOrder =
      scope === "projects"
        ? isSectionFirst(scope)
          ? "conversations_first"
          : "projects_first"
        : isSectionFirst(scope)
          ? "projects_first"
          : "conversations_first";
    onUiPreferencesChange({ sidebarSectionOrder: nextOrder });
    closeSectionMenu();
  };

  const archiveProjectCount = pendingArchiveProject
    ? visibleConversations.filter((conversation) => conversation.projectId === pendingArchiveProject.id).length
    : 0;

  const renderConversationRow = (conversation: ChatConversation, nested = false) => (
    <ConversationRow
      activeConversationId={activeConversationId}
      archiveLabel={t("conversation.archiveConversation")}
      conversation={conversation}
      key={conversation.id}
      language={language}
      markUnreadLabel={t("conversation.markUnread")}
      nested={nested}
      now={now}
      onArchiveConversation={onArchiveConversation}
      onMarkConversationUnread={onMarkConversationUnread}
      onRenameConversation={startRenamingConversation}
      onSelectConversation={onSelectConversation}
      onTogglePinConversation={onTogglePinConversation}
      pinLabel={t("conversation.pinConversation")}
      processingLabel={t("sidebar.processingConversation")}
      renameLabel={t("conversation.renameConversation")}
      unreadLabel={t("sidebar.unreadConversation")}
      unpinLabel={t("conversation.unpinConversation")}
      waitingApprovalLabel={t("sidebar.waitingApproval")}
      justNow={t("sidebar.justNow")}
    />
  );

  const renderConversationCollection = (
    listKey: string,
    collection: ChatConversation[],
    nested = false,
  ) => {
    const stage = conversationListStages[listKey] ?? "collapsed";
    const shouldPaginate = collection.length > 7;
    const visibleCount = !shouldPaginate || stage === "expanded"
      ? collection.length
      : stage === "preview"
        ? PREVIEW_CONVERSATION_COUNT
        : COLLAPSED_CONVERSATION_COUNT;
    const visibleCollection = collection.slice(0, visibleCount);
    const canExpand = visibleCollection.length < collection.length;
    const canCollapse = shouldPaginate && stage !== "collapsed";

    const expandCollection = () => {
      setConversationListStages((currentStages) => ({
        ...currentStages,
        [listKey]: stage === "collapsed" && collection.length > PREVIEW_CONVERSATION_THRESHOLD
          ? "preview"
          : "expanded",
      }));
    };

    const collapseCollection = () => {
      setConversationListStages((currentStages) => ({
        ...currentStages,
        [listKey]: "collapsed",
      }));
    };

    return (
      <>
        {visibleCollection.map((conversation) => renderConversationRow(conversation, nested))}
        {shouldPaginate && (canExpand || canCollapse) && (
          <div
            className="left-sidebar__conversation-disclosure"
            data-nested={nested || undefined}
          >
            {canExpand && (
              <button type="button" onClick={expandCollection}>
                {t("sidebar.expandConversations")}
              </button>
            )}
            {canCollapse && (
              <button type="button" onClick={collapseCollection}>
                {t("sidebar.collapseConversations")}
              </button>
            )}
          </div>
        )}
      </>
    );
  };

  const renderSectionMenu = (scope: SidebarSectionScope) => {
    const archiveCount = scope === "projects" ? projectArchiveAllCount : rootArchiveAllCount;
    const archiveScope: BulkArchiveScope = scope === "projects" ? "projects" : "root";
    const moveDown = isSectionFirst(scope);
    const MoveIcon = moveDown ? ArrowDown : ArrowUp;

    return (
      <div className="left-sidebar__section-menu" role="menu" ref={sectionMenuRef}>
        <button
          className="left-sidebar__section-menu-item"
          type="button"
          role="menuitem"
          disabled={archiveCount === 0}
          onClick={() => {
            setPendingBulkArchiveScope(archiveScope);
            closeSectionMenu();
          }}
        >
          <Archive aria-hidden="true" />
          <span>{t("sidebar.archiveAllChats")}</span>
        </button>

        <div className="left-sidebar__section-menu-divider" />

        <button
          className="left-sidebar__section-menu-item"
          type="button"
          role="menuitem"
          data-open={openSectionSubmenu === "organize" || undefined}
          onMouseEnter={() => setOpenSectionSubmenu("organize")}
          onClick={() =>
            setOpenSectionSubmenu((currentSubmenu) =>
              currentSubmenu === "organize" ? null : "organize",
            )
          }
        >
          <NotebookText aria-hidden="true" />
          <span>{t("sidebar.organizeSidebar")}</span>
          <ChevronDown className="left-sidebar__section-menu-item__chevron" aria-hidden="true" />
        </button>

        <button
          className="left-sidebar__section-menu-item"
          type="button"
          role="menuitem"
          data-open={openSectionSubmenu === "sort" || undefined}
          onMouseEnter={() => setOpenSectionSubmenu("sort")}
          onClick={() =>
            setOpenSectionSubmenu((currentSubmenu) => (currentSubmenu === "sort" ? null : "sort"))
          }
        >
          <Clock3 aria-hidden="true" />
          <span>{t("sidebar.sortBy")}</span>
          <ChevronDown className="left-sidebar__section-menu-item__chevron" aria-hidden="true" />
        </button>

        {openSectionSubmenu === "organize" && (
          <div className="left-sidebar__section-submenu" role="menu">
            <button
              className="left-sidebar__section-menu-item"
              type="button"
              role="menuitem"
              onClick={() => setProjectSort("created")}
            >
              <NotebookText aria-hidden="true" />
              <span>{t("sidebar.organizeByProject")}</span>
              {uiPreferences.sidebarProjectSort === "created" && (
                <Check className="left-sidebar__section-menu-check" aria-hidden="true" />
              )}
            </button>
            <button
              className="left-sidebar__section-menu-item"
              type="button"
              role="menuitem"
              onClick={() => setProjectSort("recent")}
            >
              <NotebookText aria-hidden="true" />
              <span>{t("sidebar.organizeByRecentProject")}</span>
              {uiPreferences.sidebarProjectSort === "recent" && (
                <Check className="left-sidebar__section-menu-check" aria-hidden="true" />
              )}
            </button>
            <button
              className="left-sidebar__section-menu-item"
              type="button"
              role="menuitem"
              onClick={() => toggleSectionOrder(scope)}
            >
              <MoveIcon aria-hidden="true" />
              <span>{moveDown ? t("sidebar.moveDown") : t("sidebar.moveUp")}</span>
            </button>
          </div>
        )}

        {openSectionSubmenu === "sort" && (
          <div className="left-sidebar__section-submenu" role="menu">
            <button
              className="left-sidebar__section-menu-item"
              type="button"
              role="menuitem"
              onClick={() => setConversationSort("created")}
            >
              <Clock3 aria-hidden="true" />
              <span>{t("sidebar.sortCreated")}</span>
              {uiPreferences.sidebarConversationSort === "created" && (
                <Check className="left-sidebar__section-menu-check" aria-hidden="true" />
              )}
            </button>
            <button
              className="left-sidebar__section-menu-item"
              type="button"
              role="menuitem"
              onClick={() => setConversationSort("updated")}
            >
              <PencilLine aria-hidden="true" />
              <span>{t("sidebar.sortUpdated")}</span>
              {uiPreferences.sidebarConversationSort === "updated" && (
                <Check className="left-sidebar__section-menu-check" aria-hidden="true" />
              )}
            </button>
          </div>
        )}
      </div>
    );
  };

  const renderProjectGroup = (project: AppProject, pinnedSection = false, sectionProjects: AppProject[]) => {
    const isProjectOpen = openProjectIds.has(project.id);
    const isProjectPinned = Boolean(project.pinnedAt);
    const ProjectIcon = isProjectOpen ? FolderOpen : Folder;
    const projectConversations = conversationsByProjectId[project.id] ?? [];
    const visibleProjectConversationCount = projectConversations.length;
    const isDragging = draggingProjectId === project.id;
    const canDragProject = !isProjectOpen && sectionProjects.length > 1;
    const hasPendingProjectConversation = projectConversations.some((conversation) =>
      conversation.messages.some((message) => message.role === "assistant" && message.status === "pending"),
    );
    const hasUnreadProjectConversation = projectConversations.some(
      (conversation) => conversation.unreadAt && conversation.id !== activeConversationId,
    );
    const shouldShowProjectStatus =
      !isProjectOpen && (hasPendingProjectConversation || hasUnreadProjectConversation);

    return (
      <div
        className={`left-sidebar__project-group${pinnedSection ? " left-sidebar__project-group--pinned" : ""}`}
        data-dragging={isDragging || undefined}
        key={project.id}
      >
        <div
          className="left-sidebar__project-row"
          data-project-id={project.id}
          ref={(element) => {
            if (element) {
              projectRowRefs.current.set(project.id, element);
            } else {
              projectRowRefs.current.delete(project.id);
            }
          }}
          onContextMenu={(event) => {
            event.preventDefault();
            closeSectionMenu();
            setOpenProjectMenuId(project.id);
          }}
        >
          <button
            className="left-sidebar__project-main"
            type="button"
            data-draggable={canDragProject || undefined}
            onClick={(event) => {
              if (suppressProjectClickRef.current === project.id) {
                event.preventDefault();
                return;
              }

              toggleProject(project.id);
            }}
            onPointerCancel={handleProjectPointerCancel}
            onPointerDown={(event) => handleProjectPointerDown(event, project, sectionProjects, isProjectOpen)}
            onPointerMove={handleProjectPointerMove}
            onPointerUp={handleProjectPointerUp}
          >
            <ProjectIcon className="left-sidebar__project-icon" aria-hidden="true" />
            <span>{project.name}</span>
            <ChevronDown
              className="left-sidebar__project-inline-chevron"
              data-open={isProjectOpen || undefined}
              aria-hidden="true"
            />
          </button>

          {shouldShowProjectStatus && (
            <span className="left-sidebar__project-status">
              {hasPendingProjectConversation ? (
                <span className="left-sidebar__conversation-spinner" aria-label="正在处理" />
              ) : (
                <span className="left-sidebar__conversation-unread-dot" aria-label={t("sidebar.unreadConversation")} />
              )}
            </span>
          )}

          <div className="left-sidebar__project-actions">
            <button
              className="left-sidebar__project-action left-sidebar__project-action--more"
              type="button"
              aria-label={t("project.moreActions")}
              data-menu-open={openProjectMenuId === project.id || undefined}
              onClick={() => {
                closeSectionMenu();
                if (openProjectMenuId === project.id) {
                  closeProjectMenu();
                  return;
                }

                setOpenProjectMenuId(project.id);
              }}
            >
              <MoreHorizontal aria-hidden="true" />
            </button>
            <button
              className="left-sidebar__project-action"
              type="button"
              aria-label={t("project.newProjectConversation")}
              onClick={() => onNewConversation(project.id)}
            >
              <SquarePen aria-hidden="true" />
            </button>
          </div>
        </div>

        {openProjectMenuId === project.id && (
          <div className="left-sidebar__project-menu" role="menu" ref={projectMenuRef}>
            <button
              className="left-sidebar__project-menu-item"
              type="button"
              role="menuitem"
              onClick={() => {
                onTogglePinProject(project.id);
                closeProjectMenu();
              }}
            >
              <Pin aria-hidden="true" />
              <span>{isProjectPinned ? t("project.unpinProject") : t("project.pinProject")}</span>
            </button>
            <button
              className="left-sidebar__project-menu-item"
              type="button"
              role="menuitem"
              disabled={!project.path}
              onClick={() => {
                onShowProjectInFolder(project.id);
                closeProjectMenu();
              }}
            >
              <FolderOpen aria-hidden="true" />
              <span>{t("project.showInFolder")}</span>
            </button>
            <button
              className="left-sidebar__project-menu-item"
              type="button"
              role="menuitem"
              onClick={() => startRenamingProject(project)}
            >
              <PencilLine aria-hidden="true" />
              <span>{t("project.renameProject")}</span>
            </button>
            <button
              className="left-sidebar__project-menu-item"
              type="button"
              role="menuitem"
              disabled={visibleProjectConversationCount === 0}
              onClick={() => {
                setPendingArchiveProject(project);
                closeProjectMenu();
              }}
            >
              <Archive aria-hidden="true" />
              <span>{t("project.archiveConversations")}</span>
            </button>
            <button
              className="left-sidebar__project-menu-item left-sidebar__project-menu-item--danger"
              type="button"
              role="menuitem"
              onClick={() => {
                setPendingRemoveProject(project);
                closeProjectMenu();
              }}
            >
              <X aria-hidden="true" />
              <span>{t("project.removeProject")}</span>
            </button>
          </div>
        )}

        {isProjectOpen && projectConversations.length > 0 && (
          <div className="left-sidebar__project-conversations">
            {renderConversationCollection(`project:${project.id}`, projectConversations, true)}
          </div>
        )}
      </div>
    );
  };

  const projectsSection = (
    <section
      key="projects"
      className="left-sidebar__section left-sidebar__projects"
      aria-labelledby="projects-heading"
    >
      <div className="left-sidebar__section-header">
        <button
          className="left-sidebar__section-title-button"
          type="button"
          aria-expanded={areProjectsOpen}
          onClick={() => setAreProjectsOpen((open) => !open)}
        >
          <h2 id="projects-heading" className="left-sidebar__section-title">
            {t("sidebar.projects")}
          </h2>
          <ChevronDown data-open={areProjectsOpen || undefined} aria-hidden="true" />
        </button>

        <div className="left-sidebar__section-actions">
          <button
            className="left-sidebar__section-action"
            type="button"
            aria-label={t("project.moreActions")}
            data-open={openSectionMenu === "projects" || undefined}
            onClick={() => openSectionActions("projects")}
          >
            <MoreHorizontal aria-hidden="true" />
          </button>
        </div>
      </div>

      {openSectionMenu === "projects" && renderSectionMenu("projects")}

      {areProjectsOpen && displayRegularProjects.length > 0 && (
        <div className="left-sidebar__project-list">
          {displayRegularProjects.map((project) => renderProjectGroup(project, false, displayRegularProjects))}
        </div>
      )}
    </section>
  );

  const conversationsSection = (
    <section
      key="conversations"
      className="left-sidebar__section left-sidebar__conversations"
      aria-labelledby="conversations-heading"
    >
      <div className="left-sidebar__conversation-header">
        <button
          className="left-sidebar__conversation-title-button"
          type="button"
          aria-expanded={areConversationsOpen}
          onClick={() => setAreConversationsOpen((open) => !open)}
        >
          <h2 id="conversations-heading" className="left-sidebar__section-title">
            {t("sidebar.conversations")}
          </h2>
          <ChevronDown data-open={areConversationsOpen || undefined} aria-hidden="true" />
        </button>

        <div className="left-sidebar__conversation-actions">
          <button
            className="left-sidebar__conversation-action"
            type="button"
            aria-label={t("sidebar.moreConversationActions")}
            data-open={openSectionMenu === "conversations" || undefined}
            onClick={() => openSectionActions("conversations")}
          >
            <MoreHorizontal aria-hidden="true" />
          </button>
          <button
            className="left-sidebar__conversation-action"
            type="button"
            aria-label={t("sidebar.newConversationAction")}
            onClick={() => onNewConversation(null)}
          >
            <SquarePen aria-hidden="true" />
          </button>
        </div>
      </div>

      {openSectionMenu === "conversations" && renderSectionMenu("conversations")}

      {areConversationsOpen &&
        (rootConversations.length === 0 ? (
          <p className="left-sidebar__empty-state">{t("sidebar.emptyConversations")}</p>
        ) : (
          <div className="left-sidebar__conversation-list">
            {renderConversationCollection(ROOT_CONVERSATION_LIST_KEY, rootConversations)}
          </div>
        ))}
    </section>
  );

  return (
    <aside
      className="left-sidebar"
      aria-label={t("app.leftSidebar")}
      onContextMenu={(event) => {
        if (!event.defaultPrevented) event.preventDefault();
      }}
    >
      <div className="left-sidebar__header">
        <button
          className="left-sidebar__primary-action"
          type="button"
          onClick={() => onNewConversation(null)}
        >
          <SquarePen aria-hidden="true" />
          <span>{t("sidebar.newConversation")}</span>
        </button>
      </div>

      <div className="left-sidebar__scroll">
        {hasPinnedItems && (
          <section className="left-sidebar__section left-sidebar__pinned" aria-labelledby="pinned-heading">
            <h2 id="pinned-heading" className="left-sidebar__section-title left-sidebar__standalone-title">
              {t("sidebar.pinned")}
            </h2>
            <div className="left-sidebar__pinned-list">
              {pinnedRootConversations.map((conversation) => renderConversationRow(conversation))}
              {pinnedProjects.map((project) => renderProjectGroup(project, true, []))}
            </div>
          </section>
        )}

        {uiPreferences.sidebarSectionOrder === "conversations_first" ? (
          <>
            {conversationsSection}
            {projectsSection}
          </>
        ) : (
          <>
            {projectsSection}
            {conversationsSection}
          </>
        )}
      </div>

      <div className="left-sidebar__footer" data-menu-open={isAccountMenuOpen || undefined} ref={accountMenuRef}>
        {isAccountMenuOpen && (
          <div className="left-sidebar__account-menu" role="menu" aria-label={t("sidebar.accountMenu")}>
            <div className="left-sidebar__account-menu-profile" aria-hidden="true">
              <span className="left-sidebar__account-avatar left-sidebar__account-avatar--small">
                {uiPreferences.profileAvatarDataUrl ? (
                  <img src={uiPreferences.profileAvatarDataUrl} alt="" />
                ) : (
                  <span>{profileInitials}</span>
                )}
              </span>
              <span className="left-sidebar__account-menu-profile-text">
                <span>{profileDisplayName}</span>
                <span>@{profileHandle}</span>
              </span>
            </div>

            <div className="left-sidebar__account-menu-divider" />

            <button
              className="left-sidebar__account-menu-item"
              type="button"
              role="menuitem"
              onClick={() => {
                setAccountMenuOpen(false);
                onOpenSettings();
              }}
            >
              <Settings aria-hidden="true" />
              <span>{t("profile.openSettings")}</span>
            </button>
          </div>
        )}

        <button
          className="left-sidebar__account-button"
          type="button"
          aria-label={t("sidebar.accountMenu")}
          aria-expanded={isAccountMenuOpen}
          onClick={() => setAccountMenuOpen((isOpen) => !isOpen)}
        >
          <span className="left-sidebar__account-avatar">
            {uiPreferences.profileAvatarDataUrl ? (
              <img src={uiPreferences.profileAvatarDataUrl} alt="" />
            ) : (
              <span>{profileInitials}</span>
            )}
          </span>
          <span className="left-sidebar__account-text">
            <span>{profileDisplayName}</span>
            <span>@{profileHandle}</span>
          </span>
        </button>
      </div>

      {renamingProject && (
        <TextInputDialog
          title={t("project.renameTitle")}
          description={t("project.renameDescription")}
          value={renameValue}
          confirmDisabled={!renameValue.trim()}
          cancelLabel={t("project.cancel")}
          confirmLabel={t("project.save")}
          onCancel={() => setRenamingProject(null)}
          onConfirm={confirmRenameProject}
          onValueChange={setRenameValue}
        />
      )}

      {renamingConversation && (
        <TextInputDialog
          title={t("conversation.renameTitle")}
          description={t("conversation.renameDescription")}
          value={conversationRenameValue}
          confirmDisabled={!conversationRenameValue.trim()}
          cancelLabel={t("project.cancel")}
          confirmLabel={t("project.save")}
          onCancel={() => setRenamingConversation(null)}
          onConfirm={confirmRenameConversation}
          onValueChange={setConversationRenameValue}
        />
      )}

      {pendingBulkArchiveScope && (
        <ConfirmationDialog
          title={formatTemplate(t("sidebar.archiveAllTitle"), { count: bulkArchiveCount })}
          description={
            pendingBulkArchiveScope === "projects"
              ? t("sidebar.archiveAllProjectsDescription")
              : t("sidebar.archiveAllRootDescription")
          }
          cancelLabel={t("project.cancel")}
          confirmLabel={t("project.archiveAll")}
          onCancel={() => setPendingBulkArchiveScope(null)}
          onConfirm={() => {
            if (pendingBulkArchiveScope === "projects") {
              onArchiveAllProjectConversations();
            } else {
              onArchiveAllRootConversations();
            }
            setPendingBulkArchiveScope(null);
          }}
        />
      )}

      {pendingArchiveProject && (
        <ConfirmationDialog
          title={formatTemplate(t("project.archiveTitle"), { count: archiveProjectCount })}
          description={formatTemplate(t("project.archiveDescription"), {
            projectName: pendingArchiveProject.name,
          })}
          cancelLabel={t("project.cancel")}
          confirmLabel={t("project.archiveAll")}
          onCancel={() => setPendingArchiveProject(null)}
          onConfirm={() => {
            onArchiveProjectConversations(pendingArchiveProject.id);
            setPendingArchiveProject(null);
          }}
        />
      )}

      {pendingRemoveProject && (
        <ConfirmationDialog
          title={formatTemplate(t("project.removeTitle"), { projectName: pendingRemoveProject.name })}
          description={t("project.removeDescription")}
          cancelLabel={t("project.cancel")}
          confirmLabel={t("project.confirmRemove")}
          onCancel={() => setPendingRemoveProject(null)}
          onConfirm={() => {
            onRemoveProject(pendingRemoveProject.id);
            setPendingRemoveProject(null);
          }}
        />
      )}
    </aside>
  );
}
