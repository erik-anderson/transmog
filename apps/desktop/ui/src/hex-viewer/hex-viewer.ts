import { WebUIElement, observable } from '@microsoft/webui-framework';
import { decodeBytes, formatHexSelection, utf8Selection, type HexCopyFormat } from './hex-formats.js';

export interface HexPreview { key: string; bytesBase64: string; offset: number; truncated: boolean; }
interface HexCell { index: number; hex: string; text: string; hexId: string; textId: string; selection: string; }
interface HexRow { index: number; address: string; top: string; cells: HexCell[]; }
type Column = 'hex' | 'text';
let nextViewer = 0;

export class HexViewer extends WebUIElement {
  @observable preview: HexPreview | null = null;
  @observable hexRows: HexRow[] = [];
  @observable canvasHeight = '0px';
  @observable caret = -1;
  @observable rangeStart = -1;
  @observable rangeEnd = -1;
  @observable activeColumn: Column = 'hex';
  @observable activeCellId = '';
  @observable selectionSummary = 'No bytes selected';
  @observable selectAllLabel = 'Select all';
  @observable utf8Available = false;
  @observable copyMenuOpen = false;
  @observable hexMenuX = '0px';
  @observable hexMenuY = '0px';
  @observable menuSide = 'right';
  viewport!: HTMLDivElement;
  columnHeader!: HTMLDivElement;
  hexHeader!: HTMLDivElement;
  textHeader!: HTMLDivElement;
  contextMenu!: HTMLDivElement;
  copyMenu!: HTMLDivElement;
  copyAsButton!: HTMLButtonElement;
  private bytes: Uint8Array = new Uint8Array();
  private anchor = -1;
  private rowHeight = 19;
  private firstRow = -1;
  private lastRow = -1;
  private readonly viewerId = 'hex-viewer-' + nextViewer++;
  private resizeObserver: ResizeObserver | null = null;
  private pointer: number | null = null;
  private pointerX = 0;
  private pointerY = 0;
  private scrollTimer: ReturnType<typeof setInterval> | null = null;
  private readonly outsideMove = (event: PointerEvent): void => {
    if (!event.composedPath().includes(this.viewport)) this.moveSelection(event);
  };
  private readonly outsideUp = (event: PointerEvent): void => { this.finishSelection(event); };
  private readonly outsideCancel = (): void => { this.stopDrag(); };

  protected hydratedCallback(): void {
    this.resizeObserver = new ResizeObserver(() => this.renderRows(true));
    this.resizeObserver.observe(this.viewport);
    this.renderRows(true);
  }

  previewChanged(previous: HexPreview | null | undefined): void {
    const reset = previous?.key !== this.preview?.key || previous?.offset !== this.preview?.offset;
    this.selectAllLabel = this.preview?.truncated ? 'Select all (truncated)' : 'Select all';
    if (previous?.bytesBase64 !== this.preview?.bytesBase64 || reset) {
      this.bytes = this.preview ? decodeBytes(this.preview.bytesBase64) : new Uint8Array();
      if (reset) {
        this.stopDrag(); this.closeMenu(false);
        this.viewport.scrollTop = 0;
        this.caret = this.bytes.length ? 0 : -1;
        this.anchor = this.caret;
        this.rangeStart = this.rangeEnd = -1;
      } else {
        this.caret = Math.min(this.caret, this.bytes.length - 1);
        this.anchor = Math.min(this.anchor, this.bytes.length - 1);
        this.rangeEnd = Math.min(this.rangeEnd, this.bytes.length - 1);
        if (this.rangeStart > this.rangeEnd) this.rangeStart = this.rangeEnd = -1;
      }
      this.renderRows(true);
      this.describeSelection();
    }
  }

  renderRows(force = false): void {
    if (!this.viewport || !this.columnHeader) return;
    this.rowHeight = this.columnHeader.getBoundingClientRect().height || this.rowHeight;
    const rowCount = Math.ceil(this.bytes.length / 16);
    this.canvasHeight = rowCount * this.rowHeight + 'px';
    const first = Math.max(0, Math.floor(this.viewport.scrollTop / this.rowHeight) - 3);
    const last = Math.min(rowCount, first + Math.ceil(this.viewport.clientHeight / this.rowHeight) + 7);
    if (!force && first === this.firstRow && last === this.lastRow) return;
    this.firstRow = first; this.lastRow = last;
    const rows: HexRow[] = [];
    for (let row = first; row < last; row++) {
      const cells: HexCell[] = [];
      for (let index = row * 16; index < Math.min(this.bytes.length, (row + 1) * 16); index++) {
        const byte = this.bytes[index]!;
        cells.push({ index, hex: byte.toString(16).padStart(2, '0').toUpperCase(),
          text: byte >= 32 && byte <= 126 ? String.fromCharCode(byte) : '.',
          hexId: this.cellId(index, 'hex'), textId: this.cellId(index, 'text'), selection: String(index >= this.rangeStart && index <= this.rangeEnd) });
      }
      rows.push({ index: row, address: this.offset(row * 16), top: row * this.rowHeight + 'px', cells });
    }
    this.hexRows = rows;
  }

  private cellId(index: number, column: Column): string { return this.viewerId + '-' + column + '-' + index; }
  private offset(index: number): string { return ((this.preview?.offset ?? 0) + index).toString(16).padStart(8, '0').toUpperCase(); }
  private selectedBytes(): Uint8Array { return this.rangeStart < 0 ? new Uint8Array() : this.bytes.subarray(this.rangeStart, this.rangeEnd + 1); }
  private nativeSelection(): boolean { return !!window.getSelection()?.toString(); }
  private describeSelection(): void {
    this.hexRows = this.hexRows.map(row => ({ ...row, cells: row.cells.map(cell => ({ ...cell, selection: String(cell.index >= this.rangeStart && cell.index <= this.rangeEnd) })) }));
    this.activeCellId = this.caret < 0 ? '' : this.cellId(this.caret, this.activeColumn);
    this.selectionSummary = this.rangeStart < 0
      ? this.caret < 0 ? 'No bytes selected' : 'Offset ' + this.offset(this.caret) + ' · No bytes selected'
      : this.offset(this.rangeStart) + '–' + this.offset(this.rangeEnd) + ' · ' + (this.rangeEnd - this.rangeStart + 1).toLocaleString() + ' bytes selected';
    this.utf8Available = this.rangeStart >= 0 && utf8Selection(this.selectedBytes()) !== null;
  }
  private selectTo(index: number, extend: boolean): void {
    if (!this.bytes.length) return;
    const next = Math.max(0, Math.min(this.bytes.length - 1, index));
    if (!extend || this.anchor < 0) this.anchor = next;
    this.caret = next;
    this.rangeStart = Math.min(this.anchor, next);
    this.rangeEnd = Math.max(this.anchor, next);
    this.describeSelection();
  }

  startSelection(event: PointerEvent): void {
    if (event.button !== 0) return;
    const cell = (event.target as Element).closest<HTMLElement>('[data-byte]');
    if (!cell) return;
    event.preventDefault(); this.closeMenu(false);
    window.getSelection()?.removeAllRanges();
    this.activeColumn = cell.dataset.column as Column;
    if (event.shiftKey && this.rangeStart < 0) this.anchor = this.caret;
    this.selectTo(Number(cell.dataset.byte), event.shiftKey);
    this.viewport.focus({ preventScroll: true });
    this.pointer = event.pointerId;
    this.pointerX = event.clientX; this.pointerY = event.clientY;
    this.viewport.setPointerCapture(event.pointerId);
    window.addEventListener('pointermove', this.outsideMove);
    window.addEventListener('pointerup', this.outsideUp);
    window.addEventListener('pointercancel', this.outsideCancel);
    this.scrollTimer = setInterval(() => this.dragScroll(), 30);
  }
  moveSelection(event: PointerEvent): void {
    if (this.pointer !== event.pointerId) return;
    this.pointerX = event.clientX; this.pointerY = event.clientY;
    this.selectPointerByte();
  }
  finishSelection(event: PointerEvent): void {
    if (this.pointer !== event.pointerId) return;
    this.moveSelection(event); this.stopDrag();
  }
  cancelSelection(): void { this.stopDrag(); }
  lostCapture(event: PointerEvent): void {
    if (this.pointer === event.pointerId && !(event.buttons & 1)) this.stopDrag();
  }
  private stopDrag(): void {
    const pointer = this.pointer; this.pointer = null;
    if (pointer !== null && this.viewport?.hasPointerCapture(pointer)) this.viewport.releasePointerCapture(pointer);
    if (this.scrollTimer !== null) clearInterval(this.scrollTimer);
    this.scrollTimer = null;
    window.removeEventListener('pointermove', this.outsideMove);
    window.removeEventListener('pointerup', this.outsideUp);
    window.removeEventListener('pointercancel', this.outsideCancel);
  }
  private selectPointerByte(): void {
    const bounds = this.viewport.getBoundingClientRect();
    const header = (this.activeColumn === 'hex' ? this.hexHeader : this.textHeader).getBoundingClientRect();
    const column = Math.max(0, Math.min(15, Math.floor((this.pointerX - header.left) / (header.width / 16))));
    const y = Math.max(bounds.top + this.rowHeight, Math.min(bounds.bottom - 1, this.pointerY));
    const row = Math.max(0, Math.floor((y - bounds.top + this.viewport.scrollTop - this.rowHeight) / this.rowHeight));
    this.selectTo(row * 16 + column, true);
  }
  private dragScroll(): void {
    if (this.pointer === null) return;
    const bounds = this.viewport.getBoundingClientRect();
    const direction = this.pointerY < bounds.top + this.rowHeight + 18 ? -1 : this.pointerY > bounds.bottom - 18 ? 1 : 0;
    if (!direction) return;
    this.viewport.scrollTop += direction * this.rowHeight;
    this.renderRows(); this.selectPointerByte();
  }

  keyboard(event: KeyboardEvent): void {
    if (!this.bytes.length || this.nativeSelection()) return;
    const control = event.ctrlKey || event.metaKey;
    if (control && event.key.toLowerCase() === 'c') {
      event.preventDefault();
      if (event.altKey) this.openMenu(undefined, true);
      else void this.copySelection('base64');
      return;
    }
    if (control && event.key.toLowerCase() === 'a') { event.preventDefault(); this.selectAll(); return; }
    if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) { event.preventDefault(); this.openMenu(); return; }
    if (event.key === 'Tab' && !control && !event.altKey) {
      if ((!event.shiftKey && this.activeColumn === 'hex') || (event.shiftKey && this.activeColumn === 'text')) {
        event.preventDefault(); this.activeColumn = this.activeColumn === 'hex' ? 'text' : 'hex'; this.describeSelection();
      }
      return;
    }
    if (event.key === 'Escape') { event.preventDefault(); this.stopDrag(); this.rangeStart = this.rangeEnd = -1; this.anchor = this.caret; this.describeSelection(); return; }
    if (event.altKey) return;
    const current = Math.max(0, this.caret);
    const page = Math.max(1, Math.floor(this.viewport.clientHeight / this.rowHeight) - 1) * 16;
    let next: number;
    switch (event.key) {
      case 'ArrowLeft': next = current - 1; break;
      case 'ArrowRight': next = current + 1; break;
      case 'ArrowUp': next = current - 16; break;
      case 'ArrowDown': next = current + 16; break;
      case 'Home': next = control ? 0 : Math.floor(current / 16) * 16; break;
      case 'End': next = control ? this.bytes.length - 1 : Math.floor(current / 16) * 16 + 15; break;
      case 'PageUp': next = current - page; break;
      case 'PageDown': next = current + page; break;
      default: return;
    }
    event.preventDefault(); event.stopPropagation(); this.stopDrag();
    next = Math.max(0, Math.min(this.bytes.length - 1, next));
    if (event.shiftKey) {
      if (this.rangeStart < 0) this.anchor = current;
      this.selectTo(next, true);
    } else {
      this.caret = this.anchor = next;
      this.rangeStart = this.rangeEnd = -1;
      this.describeSelection();
    }
    this.revealCaret();
  }
  private revealCaret(): void {
    const top = Math.floor(this.caret / 16) * this.rowHeight;
    const available = this.viewport.clientHeight - this.rowHeight;
    if (top < this.viewport.scrollTop) this.viewport.scrollTop = top;
    else if (top + this.rowHeight > this.viewport.scrollTop + available) this.viewport.scrollTop = top + this.rowHeight - available;
    this.renderRows();
  }
  selectAll(): void {
    if (!this.bytes.length) return;
    this.anchor = 0; this.selectTo(this.bytes.length - 1, true);
    this.closeMenu(); this.revealCaret();
  }
  copyEvent(event: ClipboardEvent): void {
    if (this.nativeSelection() || this.rangeStart < 0 || !event.clipboardData) return;
    event.clipboardData.setData('text/plain', formatHexSelection(this.selectedBytes(), 'base64')!);
    event.preventDefault();
  }
  async copySelection(format: HexCopyFormat): Promise<void> {
    if (this.rangeStart < 0) return;
    const text = formatHexSelection(this.selectedBytes(), format);
    if (text === null) return;
    await this.writeClipboard(text, 'Selected bytes copied.');
  }
  async copyOffset(format: string): Promise<void> {
    if (this.caret < 0) return;
    const offset = (this.preview?.offset ?? 0) + this.caret;
    await this.writeClipboard(format === 'hex' ? '0x' + offset.toString(16) : String(offset), 'Offset copied.');
  }
  private async writeClipboard(text: string, message: string): Promise<void> {
    try {
      const copying = navigator.clipboard.writeText(text);
      this.closeMenu();
      await copying; this.$emit('diagnostic', message);
    } catch { this.closeMenu(); this.$emit('diagnostic', 'Copy failed. Try Ctrl+C with the byte viewer focused.'); }
  }

  context(event: MouseEvent): void {
    const cell = (event.target as Element).closest<HTMLElement>('[data-byte]');
    if (!cell) return;
    event.preventDefault(); event.stopPropagation();
    const index = Number(cell.dataset.byte);
    if (index < this.rangeStart || index > this.rangeEnd) {
      this.activeColumn = cell.dataset.column as Column;
      this.selectTo(index, false);
    }
    window.getSelection()?.removeAllRanges();
    this.viewport.focus({ preventScroll: true }); this.openMenu(event);
  }
  private openMenu(event?: MouseEvent, submenu = false): void {
    if (!event) this.revealCaret();
    this.$flushUpdates();
    const root = this.getRootNode() as Document | ShadowRoot;
    const cell = root.getElementById(this.activeCellId)?.getBoundingClientRect() ?? this.viewport.getBoundingClientRect();
    const x = event?.clientX ?? cell.left;
    this.menuSide = x + 460 > window.innerWidth ? 'left' : 'right';
    this.hexMenuX = Math.max(this.menuSide === 'left' ? Math.min(220, window.innerWidth - 240) : 8, Math.min(x, window.innerWidth - 240)) + 'px';
    this.hexMenuY = Math.max(8, Math.min(event?.clientY ?? cell.bottom, window.innerHeight - 310)) + 'px';
    this.copyMenuOpen = submenu; this.$flushUpdates(); this.contextMenu.showPopover();
    (submenu ? this.copyMenu : this.contextMenu).querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus();
  }
  showCopyMenu(focus = false): void {
    this.copyMenuOpen = true; this.$flushUpdates();
    if (focus) this.copyMenu.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
  }
  hideCopyMenu(): void { this.copyMenuOpen = false; }
  private closeMenu(restoreFocus = true): void {
    this.copyMenuOpen = false;
    if (this.contextMenu?.matches(':popover-open')) {
      this.contextMenu.hidePopover();
      if (restoreFocus) this.viewport.focus({ preventScroll: true });
    }
  }
  menuKeyboard(event: KeyboardEvent): void {
    const target = event.target as HTMLButtonElement;
    const inSubmenu = this.copyMenu.contains(target);
    if (event.key === 'Escape' || (inSubmenu && event.key === 'ArrowLeft')) {
      event.preventDefault(); event.stopPropagation();
      if (this.copyMenuOpen) { this.hideCopyMenu(); this.copyAsButton.focus(); }
      else this.closeMenu();
      return;
    }
    if (target === this.copyAsButton && ['ArrowRight', 'Enter', ' '].includes(event.key)) {
      event.preventDefault(); this.showCopyMenu(true); return;
    }
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const menu = inSubmenu ? this.copyMenu : this.contextMenu;
    const buttons = Array.from(menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')).filter(button => inSubmenu || !this.copyMenu.contains(button));
    const current = buttons.indexOf(target);
    const index = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (current + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length;
    buttons[index]?.focus();
  }
  menuToggle(event: ToggleEvent): void { if (event.newState === 'closed') this.copyMenuOpen = false; }

  disconnectedCallback(): void {
    this.stopDrag(); this.resizeObserver?.disconnect(); this.resizeObserver = null;
    super.disconnectedCallback();
  }
}
HexViewer.define('hex-viewer');
