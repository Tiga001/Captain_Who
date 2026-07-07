// Renderer UI.
export function isMacOS() {
  if (typeof navigator === "undefined") return false;

  const platform = navigator.platform.toLowerCase();
  const userAgent = navigator.userAgent.toLowerCase();

  return platform.includes("mac") || userAgent.includes("mac os x");
}
