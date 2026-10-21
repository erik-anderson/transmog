/** Clipboard text is bounded independently of source bytes and display previews. */
export const COPY_TEXT_BYTES = 16 * 1024 * 1024;
/** Only app-authored static copy errors may be shown in the menu. */
export class CopyError extends Error {}
export interface CopyAction {
  id: string;
  label: string;
  disabled?: boolean;
  read: (current: () => boolean) => string | Promise<string>;
  convert?: boolean;
}
export interface CopyRequest {
  source: HTMLElement;
  event: MouseEvent | KeyboardEvent;
  actions: CopyAction[];
  current: () => boolean;
  note?: string | undefined;
}
export function copyGesture(event: MouseEvent | KeyboardEvent): boolean {
  return !event.defaultPrevented && (!(event instanceof KeyboardEvent) ||
    !event.repeat && (event.key === 'ContextMenu' || event.key === 'F10' && event.shiftKey));
}
export function requestCopy(event: MouseEvent | KeyboardEvent, request: Omit<CopyRequest, 'event'>): void {
  if (!copyGesture(event)) return;
  event.preventDefault(); event.stopPropagation();
  request.source.dispatchEvent(new CustomEvent<CopyRequest>('inspection-copy-request', {
    bubbles: true, composed: true, detail: {...request, event},
  }));
}
/** Keep one property per line, retaining literal backslashes unambiguously. */
export function copyScalar(value: string): string {
  return value.replace(/[\\\u0000-\u001f\u007f-\u009f\u2028\u2029]/g, character => {
    if (character === '\\') return '\\\\';
    if (character === '\n') return '\\n';
    if (character === '\r') return '\\r';
    if (character === '\t') return '\\t';
    return '\\u' + character.charCodeAt(0).toString(16).padStart(4, '0');
  });
}
export function boundedCopy(text: string): string {
  if (text.length > COPY_TEXT_BYTES || new TextEncoder().encode(text).length > COPY_TEXT_BYTES)
    throw new CopyError('Copy exceeds the 16 MiB clipboard text limit. The clipboard was not changed.');
  return text;
}
export function fieldCopyActions(name: string, read: CopyAction['read'] | null): CopyAction[] {
  return [
    {id: 'name', label: 'Copy name', read: () => copyScalar(name)},
    {id: 'value', label: 'Copy value', disabled: !read, read: async current => copyScalar(await read!(current))},
    {id: 'pair', label: 'Copy name and value', disabled: !read, read: async current => copyScalar(name) + ': ' + copyScalar(await read!(current))},
  ];
}
export interface CopyTreeNode { name: string; value?: string | null; children?: CopyTreeNode[] }
export function copyTree(root: CopyTreeNode): string {
  const lines: string[] = []; let size = 0, count = 0;
  const visit = (node: CopyTreeNode, depth: number): void => {
    if (depth > 32 || ++count > 10000) throw new CopyError('The subtree exceeds the copy depth or node limit.');
    const line = '  '.repeat(depth) + copyScalar(node.name) +
      (node.value == null ? '' : ': ' + copyScalar(node.value));
    size += new TextEncoder().encode(line).length + 1;
    if (size > COPY_TEXT_BYTES) throw new CopyError('The subtree exceeds the 16 MiB clipboard text limit.');
    lines.push(line);
    for (const child of node.children ?? []) visit(child, depth + 1);
  };
  visit(root, 0); return lines.join('\n');
}
/** A menu inside a modal must share that modal's top-layer/inert boundary. */
export function copyDialog(element: HTMLElement): HTMLDialogElement | null {
  let node: Element | null = element;
  while (node) {
    if (node instanceof HTMLDialogElement) return node;
    node = node.parentElement ?? (node.getRootNode() instanceof ShadowRoot ? (node.getRootNode() as ShadowRoot).host : null);
  }
  return null;
}
