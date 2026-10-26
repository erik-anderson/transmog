export interface ConverterSelection {
  text: string;
  source: HTMLElement;
  range?: Range;
  start?: number;
  end?: number;
  inputValue?: string;
  operation?: string;
}
export interface ConverterText {
  text: string;
  operation?: string | null;
}

export function selectedConverterText(
  event: MouseEvent | KeyboardEvent,
): ConverterSelection | null {
  const target = event.composedPath().find((node) => node instanceof HTMLElement) as
    HTMLElement | undefined;
  if (!target) {
    return null;
  }
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
    if (target instanceof HTMLInputElement && target.type === "password") {
      return null;
    }
    const start = target.selectionStart,
      end = target.selectionEnd;
    return start !== null && end !== null && start !== end
      ? {
          text: target.value.slice(start, end),
          source: target,
          start,
          end,
          inputValue: target.value,
        }
      : null;
  }
  const root = target.getRootNode() as Document & { getSelection?: () => Selection | null };
  const selection = root.getSelection?.() ?? document.getSelection();
  if (!selection || selection.isCollapsed || !selection.rangeCount) {
    return null;
  }
  const range = selection.getRangeAt(0);
  const keyboard =
    event instanceof KeyboardEvent || (event instanceof MouseEvent && event.button !== 2);
  const relevant =
    range.intersectsNode(target) &&
    (keyboard ||
      [...range.getClientRects()].some(
        (rect) =>
          event instanceof MouseEvent &&
          event.clientX >= rect.left &&
          event.clientX <= rect.right &&
          event.clientY >= rect.top &&
          event.clientY <= rect.bottom,
      ));
  return relevant
    ? { text: selection.toString(), source: target, range: range.cloneRange() }
    : null;
}

export function restoreConverterSelection(selection: ConverterSelection | null): void {
  if (!selection?.source.isConnected) {
    return;
  }
  focusConverterSource(selection.source);
  if (
    selection.source instanceof HTMLInputElement ||
    selection.source instanceof HTMLTextAreaElement
  ) {
    if (selection.start !== undefined && selection.end !== undefined) {
      selection.source.setSelectionRange(selection.start, selection.end);
    }
  } else if (selection.range) {
    const root = selection.source.getRootNode() as Document & {
      getSelection?: () => Selection | null;
    };
    const current = root.getSelection?.() ?? document.getSelection();
    current?.removeAllRanges();
    current?.addRange(selection.range);
  }
}

export function focusConverterSource(source: HTMLElement | null): boolean {
  if (!source?.isConnected) {
    return false;
  }
  const target =
    source.closest<HTMLElement>(
      "input,textarea,select,button,a[href],[tabindex],[contenteditable]",
    ) ?? source;
  target.focus({ preventScroll: true });
  const root = target.getRootNode();
  return (root instanceof ShadowRoot ? root.activeElement : document.activeElement) === target;
}

export function requestFormatConverter(
  source: HTMLElement,
  text: string,
  operation?: string | null,
): void {
  source.dispatchEvent(
    new CustomEvent("format-converter-request", {
      bubbles: true,
      composed: true,
      detail: { text, source, ...(operation ? { operation } : {}) },
    }),
  );
}
