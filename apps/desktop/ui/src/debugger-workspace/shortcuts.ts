/** Reserve the debugger toggle before browser and Monaco F12 actions. */
export function debuggerToggleKey(event: KeyboardEvent, toggle: () => Promise<void>): boolean {
  if (
    event.key !== "F12" ||
    event.ctrlKey ||
    event.metaKey ||
    event.altKey ||
    event.shiftKey ||
    event.isComposing ||
    event.defaultPrevented
  ) {
    return false;
  }
  event.preventDefault();
  event.stopPropagation();
  if (!event.repeat) {
    void toggle();
  }
  return true;
}
