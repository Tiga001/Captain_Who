// Renderer UI.
export type Side = "left" | "right";
export type AppView = "workspace" | "settings";
export type WorkspaceView = "newConversation" | "conversation";

export type ActiveRunBinding = {
  conversationId: string;
  pendingMessageId: string;
};
