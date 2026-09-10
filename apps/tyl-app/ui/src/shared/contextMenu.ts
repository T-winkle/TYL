/** Disable WebView/browser context menus while preserving normal left-click selection. */
export function installContextMenuGuard(): () => void {
  const prevent = (event: MouseEvent) => event.preventDefault();
  document.addEventListener("contextmenu", prevent);
  return () => document.removeEventListener("contextmenu", prevent);
}
