import {copyGesture, copyScalar, copyTree, fieldCopyActions, type CopyRequest, type CopyTreeNode} from './inspection-copy.js';
import {selectedConverterText} from './converter-selection.js';

/** Opted-in read-only evidence only. Editors and native-value providers keep ownership. */
export function surfaceCopy(event: MouseEvent | KeyboardEvent): Omit<CopyRequest, 'event'> | null {
  if (!copyGesture(event) || selectedConverterText(event)) return null;
  const path = event.composedPath().filter((item): item is HTMLElement => item instanceof HTMLElement);
  if (!path.some(item => item.hasAttribute('data-copy-surface')) || path.some(item =>
    item.matches('input,select,textarea:not([readonly]),[contenteditable="true"],.monaco-editor,hex-viewer,auth-inspector,[role="menu"]'))) return null;
  const target = path[0]; if (!target) return null;
  const field = target.closest<HTMLElement>('dd,dt'), cell = target.closest<HTMLElement>('td,th');
  const text = target.closest<HTMLElement>('pre,code,textarea[readonly],[data-copy-text]');
  const source = field ?? cell ?? text;
  if (!source) return null;
  const snapshot = source.textContent, textValue = source instanceof HTMLTextAreaElement ? source.value : null;
  const current = (): boolean => source.isConnected && source.getClientRects().length > 0 &&
    source.textContent === snapshot && (!(source instanceof HTMLTextAreaElement) || source.value === textValue);
  if (field) {
    const definition = definitionPair(field); if (!definition) return null;
    const actions = fieldCopyActions(definition.name, () => definition.value), dl = field.closest('dl');
    if (dl) {
      const section = {name: sectionName(dl), children: [...dl.querySelectorAll('dt')]
        .filter(dt => dt.closest('dl') === dl).map(dt => definitionPair(dt)).filter((pair): pair is {name: string; value: string} => !!pair)};
      const original = dl.textContent;
      if (section.children.length > 1) actions.push({id: 'section', label: 'Copy section', read: () => copyTree(section)});
      return {source, actions, current: () => current() && dl.textContent === original};
    }
    return {source, actions, current};
  }
  if (cell) {
    const row = cell.closest('tr'), table = cell.closest('table');
    if (!row || !table || row.closest('thead')) return null;
    const cells = [...row.cells], index = cells.indexOf(cell as HTMLTableCellElement);
    const headings = [...(table.tHead?.rows[0]?.cells ?? [])].map(item => item.textContent?.trim() ?? '');
    const rowName = row.querySelector('th')?.textContent?.trim();
    const name = (rowName && index > 0 ? rowName + ' · ' : '') + (headings[index] || 'Value');
    const actions = fieldCopyActions(name, () => evidenceText(cell));
    const tree: CopyTreeNode = {name: rowName || 'Row', children: cells.map((item, column) => ({name: headings[column] || 'Value', value: evidenceText(item)}))};
    actions.push({id: 'row', label: 'Copy row', read: () => copyTree(tree)});
    const original = row.textContent;
    return {source, actions, current: () => current() && row.textContent === original};
  }
  const value = textValue ?? source.textContent ?? '';
  const label = source.getAttribute('data-copy-label') || source.getAttribute('aria-label') || 'Text';
  return {source, current, actions: [
    {id: 'text', label: source.getAttribute('data-copy-label') ? 'Copy ' + label.toLowerCase() : 'Copy text', read: () => value},
    ...(label !== 'Text' ? [
      {id: 'name', label: 'Copy name', read: () => copyScalar(label)},
      {id: 'pair', label: 'Copy name and value', read: () => copyScalar(label) + ': ' + copyScalar(value)},
    ] : []),
  ]};
}
function definitionPair(element: HTMLElement): {name: string; value: string} | null {
  let dt: Element | null = element.tagName === 'DT' ? element : element.previousElementSibling;
  while (dt && dt.tagName !== 'DT') dt = dt.previousElementSibling;
  if (!dt) return null;
  const dd = element.tagName === 'DD' ? element : dt.nextElementSibling;
  if (!dd || dd.tagName !== 'DD') return null;
  return {name: dt.textContent?.trim() ?? '', value: evidenceText(dd)};
}
function evidenceText(element: Element): string {
  return [...element.childNodes].filter(node => !(node instanceof Element) ||
    !node.matches('small,button,details,input,select,[role="status"],inspection-copy-menu'))
    .map(node => node.textContent ?? '').join('');
}
function sectionName(element: Element): string {
  return element.closest('details')?.querySelector('summary')?.textContent?.trim() ||
    element.closest('[aria-label]')?.getAttribute('aria-label') || 'Details';
}
