// Renderer UI.
import { getHostApi } from "@mycopilot/host-api";

export const hostClient = getHostApi();

export async function openExternalUrlThroughHost(url: string): Promise<void> {
  window.open(url, "_blank", "noopener,noreferrer");
}
