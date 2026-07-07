// Renderer UI.
import { openExternalUrlThroughHost } from "../host/hostClient";

export async function openExternalUrl(url: string) {
  await openExternalUrlThroughHost(url);
}
