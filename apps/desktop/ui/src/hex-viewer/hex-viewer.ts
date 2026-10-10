import { WebUIElement, observable } from '@microsoft/webui-framework';
import { formatHexSelection, utf8Selection, type HexCopyFormat } from './hex-formats.js';
import { HexByteSource, HEX_COPY_BYTES } from './hex-byte-source.js';
import { describeError } from '../utilities.js';

export interface HexPreview { key: string; bytesBase64: string; offset: number|string; truncated: boolean; source?:{sessionId:string;boundary:string;totalBytes:string;readLimit:number}; }
interface HexCell { index: string; hex: string; text: string; hexId: string; textId: string; selection: string; selected:boolean; caretHex:boolean; caretText:boolean; }
interface HexRow { index: string; address: string; top: string; cells: HexCell[]; }
type Column = 'hex' | 'text';
let nextViewer = 0;
const MAX_CANVAS_HEIGHT = 4_000_000;
const min = (a:bigint,b:bigint):bigint => a < b ? a : b;
const max = (a:bigint,b:bigint):bigint => a > b ? a : b;

export class HexViewer extends WebUIElement {
  @observable preview: HexPreview | null = null;
  @observable hexRows: HexRow[] = [];
  @observable canvasHeight = '0px';
  @observable caret = '-1';
  @observable rangeStart = '-1';
  @observable rangeEnd = '-1';
  @observable activeColumn: Column = 'hex';
  @observable activeCellId = '';
  @observable selectionSummary = 'No bytes selected';
  @observable selectAllLabel = 'Select all';
  @observable utf8Available = false;
  @observable copyMenuOpen = false;
  @observable hexMenuX = '0px';
  @observable hexMenuY = '0px';
  @observable menuSide = 'right';
  @observable wideOffsets = false;
  @observable hasCaret = false;
  @observable hasSelection = false;
  @observable byteStatus = '';
  @observable byteError = false;
  @observable offsetInput = '';
  @observable offsetError = '';
  viewport!: HTMLDivElement;
  columnHeader!: HTMLDivElement;
  hexHeader!: HTMLDivElement;
  textHeader!: HTMLDivElement;
  contextMenu!: HTMLDivElement;
  copyMenu!: HTMLDivElement;
  copyAsButton!: HTMLButtonElement;
  offsetPopover!: HTMLDivElement;
  offsetField!: HTMLInputElement;
  private source: HexByteSource | null = null;
  private anchor = -1n;
  private rowHeight = 19;
  private firstRow = -1n;
  private lastRow = -1n;
  private logicalTop = 0n;
  private nativeTop = -1;
  private wheelRows = 0;
  private copyRevision = 0;
  private copying = false;
  private validatingUtf8 = false;
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
    if (this.preview !== previous) {
      if(reset || !this.preview) { this.source?.dispose(); this.source = this.preview ? new HexByteSource(this.preview,()=>this.sourceChanged()) : null; }
      else if(this.preview) this.source?.update(this.preview);
      this.copyRevision++;
      if (reset) {
        this.stopDrag(); this.closeMenu(false);
        if(this.viewport) this.viewport.scrollTop = 0;
        this.logicalTop = 0n; this.nativeTop = 0;
        this.caret = this.length ? '0' : '-1';
        this.anchor = BigInt(this.caret);
        this.rangeStart = this.rangeEnd = '-1';
      } else {
        this.caret = min(BigInt(this.caret), this.length - 1n).toString();
        this.anchor = min(this.anchor, this.length - 1n);
        this.rangeEnd = min(BigInt(this.rangeEnd), this.length - 1n).toString();
        if (BigInt(this.rangeStart) > BigInt(this.rangeEnd)) this.rangeStart = this.rangeEnd = '-1';
      }
      this.renderRows(true);
      this.describeSelection();
    }
  }
  private get length():bigint { return this.source?.length ?? 0n; }
  private get rowCount():bigint { return (this.length + 15n) / 16n; }
  private get scaled():boolean { return Number(this.rowCount) * this.rowHeight > MAX_CANVAS_HEIGHT; }
  private get visibleRows():number { return Math.max(1,Math.floor(this.viewport.clientHeight / this.rowHeight) - 1); }
  private get maxTop():bigint { return max(0n,this.rowCount - BigInt(this.visibleRows)); }
  private sourceChanged():void {
    this.byteError = !!this.source?.error;
    this.byteStatus = this.source?.error || (this.source?.loading ? 'Loading bytes…' : '');
    this.renderRows(true); this.describeSelection();
  }
  retryBytes():void { this.source?.retry(); }
  private currentTop():bigint {
    if(!this.scaled) return BigInt(Math.max(0,Math.floor(this.viewport.scrollTop / this.rowHeight)));
    if(this.viewport.scrollTop !== this.nativeTop) {
      const extent = Math.max(1,this.viewport.scrollHeight - this.viewport.clientHeight);
      this.logicalTop = min(this.maxTop,BigInt(Math.round(this.viewport.scrollTop / extent * Number(this.maxTop))));
      this.nativeTop = this.viewport.scrollTop;
    }
    return this.logicalTop;
  }
  private scrollToRow(row:bigint):void {
    this.logicalTop = min(this.maxTop,max(0n,row));
    this.viewport.scrollTop = this.scaled
      ? Number(this.logicalTop) / Math.max(1,Number(this.maxTop)) * Math.max(0,this.viewport.scrollHeight-this.viewport.clientHeight)
      : Number(this.logicalTop) * this.rowHeight;
    this.nativeTop = this.viewport.scrollTop;
    this.renderRows(true);
  }
  wheel(event:WheelEvent):void {
    if(!this.scaled || !event.deltaY || event.ctrlKey) return;
    event.preventDefault();
    this.wheelRows += event.deltaY * (event.deltaMode === 1 ? this.rowHeight : event.deltaMode === 2 ? this.viewport.clientHeight : 1) / this.rowHeight;
    const rows = Math.trunc(this.wheelRows); this.wheelRows -= rows;
    if(rows) this.scrollToRow(this.currentTop()+BigInt(rows));
  }

  renderRows(force = false): void {
    if (!this.viewport || !this.columnHeader) return;
    this.rowHeight = this.columnHeader.getBoundingClientRect().height || this.rowHeight;
    this.canvasHeight = Math.min(MAX_CANVAS_HEIGHT, Number(this.rowCount) * this.rowHeight) + 'px';
    this.wideOffsets = this.length + BigInt(this.preview?.offset ?? 0) > 0xffffffffn;
    const top = this.currentTop();
    const first = max(0n, top - 3n);
    const last = min(this.rowCount, first + BigInt(Math.ceil(this.viewport.clientHeight / this.rowHeight) + 7));
    if (!force && first === this.firstRow && last === this.lastRow) return;
    this.firstRow = first; this.lastRow = last;
    const rows: HexRow[] = [];
    for (let row = first; row < last; row++) {
      const cells: HexCell[] = [];
      for (let index = row * 16n; index < min(this.length, (row + 1n) * 16n); index++) {
        const byte = this.source?.byte(index);
        const selected = index >= BigInt(this.rangeStart) && index <= BigInt(this.rangeEnd);
        cells.push({ index:index.toString(), hex: byte === undefined ? '··' : byte.toString(16).padStart(2, '0').toUpperCase(),
          text: byte === undefined ? '·' : byte >= 32 && byte <= 126 ? String.fromCharCode(byte) : '.',
          hexId: this.cellId(index, 'hex'), textId: this.cellId(index, 'text'), selection: String(selected), selected,
          caretHex: index === BigInt(this.caret) && this.activeColumn === 'hex', caretText: index === BigInt(this.caret) && this.activeColumn === 'text' });
      }
      rows.push({ index:row.toString(), address: this.offset(row * 16n), top: (this.scaled ? this.viewport.scrollTop + Number(row-top)*this.rowHeight : Number(row)*this.rowHeight) + 'px', cells });
    }
    this.hexRows = rows;
    this.source?.ensure(first*16n,last*16n);
  }

  private cellId(index: bigint, column: Column): string { return this.viewerId + '-' + column + '-' + index; }
  private offset(index: bigint): string { return (BigInt(this.preview?.offset ?? 0) + index).toString(16).padStart(8, '0').toUpperCase(); }
  private get selectionLength():bigint { return BigInt(this.rangeStart)<0n ? 0n : BigInt(this.rangeEnd)-BigInt(this.rangeStart)+1n; }
  private nativeSelection(): boolean { return !!window.getSelection()?.toString(); }
  private describeSelection(): void {
    this.hasCaret=BigInt(this.caret)>=0n;this.hasSelection=BigInt(this.rangeStart)>=0n;
    this.hexRows = this.hexRows.map(row => ({ ...row, cells: row.cells.map(cell => {
      const index=BigInt(cell.index),selected=index>=BigInt(this.rangeStart)&&index<=BigInt(this.rangeEnd);
      return {...cell,selected,selection:String(selected),caretHex:index===BigInt(this.caret)&&this.activeColumn==='hex',caretText:index===BigInt(this.caret)&&this.activeColumn==='text'};
    }) }));
    this.activeCellId = this.hasCaret ? this.cellId(BigInt(this.caret), this.activeColumn) : '';
    this.selectionSummary = !this.hasSelection
      ? !this.hasCaret ? 'No bytes selected' : 'Offset ' + this.offset(BigInt(this.caret)) + ' · No bytes selected'
      : this.offset(BigInt(this.rangeStart)) + '–' + this.offset(BigInt(this.rangeEnd)) + ' · ' + this.selectionLength.toLocaleString() + ' bytes selected';
    const cached=this.hasSelection&&this.selectionLength<=65536n ? this.source?.cached(BigInt(this.rangeStart),this.selectionLength) : null;
    this.utf8Available=!!cached&&utf8Selection(cached)!==null;
  }
  private selectTo(index: bigint, extend: boolean): void {
    if (!this.length) return;
    const next = max(0n, min(this.length - 1n, index));
    if (!extend || this.anchor < 0n) this.anchor = next;
    this.caret = next.toString();
    this.rangeStart = min(this.anchor, next).toString();
    this.rangeEnd = max(this.anchor, next).toString();
    this.copyRevision++;
    this.describeSelection();
  }

  startSelection(event: PointerEvent): void {
    if (event.button !== 0) return;
    const cell = (event.target as Element).closest<HTMLElement>('[data-byte]');
    if (!cell) return;
    event.preventDefault(); this.closeMenu(false);
    window.getSelection()?.removeAllRanges();
    this.activeColumn = cell.dataset.column as Column;
    if (event.shiftKey && !this.hasSelection) this.anchor = BigInt(this.caret);
    this.selectTo(BigInt(cell.dataset.byte!), event.shiftKey);
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
    const row = this.currentTop() + BigInt(Math.max(0,Math.floor((y - bounds.top - this.rowHeight) / this.rowHeight)));
    this.selectTo(row * 16n + BigInt(column), true);
  }
  private dragScroll(): void {
    if (this.pointer === null) return;
    const bounds = this.viewport.getBoundingClientRect();
    const direction = this.pointerY < bounds.top + this.rowHeight + 18 ? -1 : this.pointerY > bounds.bottom - 18 ? 1 : 0;
    if (!direction) return;
    this.scrollToRow(this.currentTop()+BigInt(direction)); this.selectPointerByte();
  }

  keyboard(event: KeyboardEvent): void {
    if (!this.length || this.nativeSelection()) return;
    const control = event.ctrlKey || event.metaKey;
    if(control && event.key.toLowerCase()==='g') { event.preventDefault(); this.openOffset(); return; }
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
    if (event.key === 'Escape') { event.preventDefault(); this.stopDrag(); this.rangeStart = this.rangeEnd = '-1'; this.anchor = BigInt(this.caret); this.copyRevision++; this.describeSelection(); return; }
    if (event.altKey) return;
    const current = max(0n, BigInt(this.caret));
    const page = BigInt(this.visibleRows) * 16n;
    let next: bigint;
    switch (event.key) {
      case 'ArrowLeft': next = current - 1n; break;
      case 'ArrowRight': next = current + 1n; break;
      case 'ArrowUp': next = current - 16n; break;
      case 'ArrowDown': next = current + 16n; break;
      case 'Home': next = control ? 0n : current / 16n * 16n; break;
      case 'End': next = control ? this.length - 1n : current / 16n * 16n + 15n; break;
      case 'PageUp': next = current - page; break;
      case 'PageDown': next = current + page; break;
      default: return;
    }
    event.preventDefault(); event.stopPropagation(); this.stopDrag();
    next = max(0n, min(this.length - 1n, next));
    if (event.shiftKey) {
      if (!this.hasSelection) this.anchor = current;
      this.selectTo(next, true);
    } else {
      this.caret = next.toString(); this.anchor = next;
      this.rangeStart = this.rangeEnd = '-1'; this.copyRevision++;
      this.describeSelection();
    }
    this.revealCaret();
  }
  private revealCaret(): void {
    const row = BigInt(this.caret) / 16n, top = this.currentTop();
    if(row<top) this.scrollToRow(row);
    else if(row>=top+BigInt(this.visibleRows)) this.scrollToRow(row-BigInt(this.visibleRows)+1n);
    else this.renderRows();
  }
  selectAll(): void {
    if (!this.length) return;
    this.anchor = 0n; this.selectTo(this.length - 1n, true);
    this.closeMenu(); this.revealCaret();
  }
  copyEvent(event: ClipboardEvent): void {
    if (this.nativeSelection() || !this.hasSelection || !event.clipboardData) return;
    const bytes=this.source?.cached(BigInt(this.rangeStart),this.selectionLength);
    if(!bytes) { event.preventDefault(); void this.copySelection('base64'); return; }
    event.clipboardData.setData('text/plain', formatHexSelection(bytes, 'base64')!);
    event.preventDefault();
  }
  async copySelection(format: HexCopyFormat): Promise<void> {
    if (!this.hasSelection || !this.source || this.copying) return;
    this.copying = true;
    const revision=this.copyRevision,source=this.source,current=()=>revision===this.copyRevision&&source===this.source;
    this.closeMenu();
    try {
      const bytes=await source.read(BigInt(this.rangeStart),this.selectionLength,current);
      if(!current())return;
      const text=formatHexSelection(bytes,format);
      if(text===null){this.$emit('diagnostic','Selected bytes are not valid UTF-8.');return;}
      await this.writeClipboard(text,'Selected bytes copied.');
    } catch(error:unknown) { if(current())this.$emit('diagnostic',describeError(error)); }
    finally {this.copying=false;}
  }
  async copyOffset(format: string): Promise<void> {
    if (!this.hasCaret) return;
    const offset = BigInt(this.preview?.offset ?? 0) + BigInt(this.caret);
    await this.writeClipboard(format === 'hex' ? '0x' + offset.toString(16) : String(offset), 'Offset copied.');
  }
  openOffset():void {
    this.closeMenu(false);this.offsetInput=this.hasCaret?(BigInt(this.preview?.offset??0)+BigInt(this.caret)).toString():'0';this.offsetError='';
    this.$flushUpdates();this.offsetField.value=this.offsetInput;this.offsetPopover.showPopover();this.offsetField.focus();this.offsetField.select();
  }
  changeOffset(event:Event):void { this.offsetInput=(event.currentTarget as HTMLInputElement).value;this.offsetError=''; }
  goOffset(event:SubmitEvent):void {
    event.preventDefault();
    try {
      if(!/^(?:\d+|0x[\da-f]+)$/i.test(this.offsetInput.trim()))throw new Error('Enter a decimal offset or hexadecimal offset beginning with 0x.');
      const offset=BigInt(this.offsetInput.trim())-BigInt(this.preview?.offset??0);
      if(offset<0n||offset>=this.length)throw new Error('The offset must be within the retained body.');
      this.caret=offset.toString();this.anchor=offset;this.rangeStart=this.rangeEnd='-1';this.copyRevision++;
      this.describeSelection();this.revealCaret();this.offsetPopover.hidePopover();this.viewport.focus({preventScroll:true});
    }catch(error:unknown){this.offsetError=describeError(error);}
  }
  offsetKeyboard(event:KeyboardEvent):void {if(event.key==='Escape'){event.preventDefault();event.stopPropagation();this.offsetPopover.hidePopover();this.viewport.focus({preventScroll:true});}}
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
    const index = BigInt(cell.dataset.byte!);
    if (index < BigInt(this.rangeStart) || index > BigInt(this.rangeEnd)) {
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
    void this.validateUtf8();
    (submenu ? this.copyMenu : this.contextMenu).querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus();
  }
  private async validateUtf8():Promise<void> {
    if(!this.hasSelection||!this.source||this.validatingUtf8||this.selectionLength>BigInt(HEX_COPY_BYTES))return;
    this.validatingUtf8=true;
    const source=this.source,revision=this.copyRevision,current=()=>source===this.source&&revision===this.copyRevision;
    try {const bytes=await source.read(BigInt(this.rangeStart),this.selectionLength,current);if(current())this.utf8Available=utf8Selection(bytes)!==null;}catch{ /* Copy reports unavailable byte ranges when requested. */ }
    finally {this.validatingUtf8=false;}
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
    this.copyRevision++;this.source?.dispose();this.source=null;
    super.disconnectedCallback();
  }
}
HexViewer.define('hex-viewer');
