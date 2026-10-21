import {WebUIElement, observable} from '@microsoft/webui-framework';
import {boundedCopy, CopyError, copyDialog, requestCopy, type CopyRequest} from '../inspection-copy.js';
import {surfaceCopy} from '../inspection-copy-surface.js';
import {requestFormatConverter} from '../converter-selection.js';

export class InspectionCopyMenu extends WebUIElement {
  @observable copyActions: Array<{id: string; label: string; disabled: boolean}> = [];
  @observable copyMenuX = '8px';
  @observable copyMenuY = '8px';
  @observable copyMenuNote = '';
  @observable copyMenuStatus = '';
  @observable copyMenuBusy = false;
  @observable copyFeedback = '';
  @observable copyColorScheme = 'light dark';
  menu!: HTMLDivElement;
  private request: CopyRequest | null = null;
  private revision = 0;
  private readonly receive = (event: Event): void => {
    const request = (event as CustomEvent<CopyRequest>).detail;
    if (copyDialog(request.source) !== copyDialog(this)) return;
    this.close(false); this.request = request;
    this.copyFeedback = ''; this.copyColorScheme = getComputedStyle(request.source).colorScheme;
    const rect = request.source.getBoundingClientRect(), pointer = request.event instanceof MouseEvent ? request.event : null;
    this.copyMenuX = Math.max(8, Math.min(pointer ? pointer.clientX : rect.left, innerWidth - 300)) + 'px';
    this.copyMenuY = Math.max(8, Math.min(pointer ? pointer.clientY : rect.bottom, innerHeight - 290)) + 'px';
    this.copyActions = request.actions.map(action => ({id: action.id, label: action.label, disabled: !!action.disabled}));
    this.copyMenuNote = request.note ?? ''; this.copyMenuStatus = ''; this.$flushUpdates();
    this.menu.showPopover({source: request.source}); this.buttons()[0]?.focus();
  };
  protected hydratedCallback(): void {
    document.addEventListener('inspection-copy-request', this.receive);
    document.addEventListener('visibilitychange', this.visibility);
    document.addEventListener('contextmenu', this.context);
    document.addEventListener('keydown', this.context);
  }
  private readonly context = (event: MouseEvent | KeyboardEvent): void => {
    // Only the menu in the originating modal builds the section snapshot.
    const target = event.composedPath().find((item): item is HTMLElement => item instanceof HTMLElement);
    if (!target || copyDialog(target) !== copyDialog(this)) return;
    const request = surfaceCopy(event);
    if (request && copyDialog(request.source) === copyDialog(this)) requestCopy(event, request);
  };
  private readonly visibility = (): void => { if (document.hidden) this.close(false); };
  private buttons(): HTMLButtonElement[] { return [...this.menu.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')]; }
  async execute(id: string): Promise<void> {
    const request = this.request, action = request?.actions.find(action => action.id === id);
    if (!request || !action || action.disabled || this.copyMenuBusy) return;
    const revision = this.revision;
    const current = (): boolean => revision === this.revision && this.request === request &&
      this.isConnected && !document.hidden && request.source.isConnected && request.source.getClientRects().length > 0 &&
      request.current() && this.menu.matches(':popover-open');
    this.copyMenuBusy = true; this.copyMenuStatus = 'Preparing copy…';
    try {
      if (!current()) { this.close(false); return; }
      const text = boundedCopy(await action.read(current));
      if (!current()) { this.close(false); return; }
      if (action.convert) { this.close(); requestFormatConverter(request.source, text); return; }
      await navigator.clipboard.writeText(text);
      if (current()) { this.copyFeedback = 'Copied.'; this.close(); }
    } catch (error: unknown) {
      if (current()) this.copyMenuStatus = error instanceof CopyError
        ? error.message : 'Copy failed. The clipboard or retained value is unavailable.';
    } finally { if (revision === this.revision) this.copyMenuBusy = false; }
  }
  close(focus = true): void {
    const source = this.request?.source; this.revision++; this.request = null; this.copyMenuBusy = false;
    if (this.menu?.matches(':popover-open')) this.menu.hidePopover();
    this.copyActions = []; this.copyMenuNote = '';
    if (focus && source?.isConnected) {
      const target = source.closest<HTMLElement>('[tabindex],button,input,textarea') ?? source;
      target.focus({preventScroll: true});
    }
  }
  toggled(event: ToggleEvent): void { if (event.newState === 'closed') this.close(false); }
  key(event: KeyboardEvent): void {
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); this.close(); return; }
    const buttons = this.buttons(), index = buttons.indexOf(this.shadowRoot?.activeElement as HTMLButtonElement);
    if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault(); event.stopPropagation();
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 :
        (index + (event.key === 'ArrowDown' ? 1 : buttons.length - 1)) % buttons.length;
      buttons[next]?.focus();
    }
  }
  disconnectedCallback(): void {
    this.close(false); document.removeEventListener('inspection-copy-request', this.receive);
    document.removeEventListener('visibilitychange', this.visibility); super.disconnectedCallback();
    document.removeEventListener('contextmenu', this.context); document.removeEventListener('keydown', this.context);
  }
}
InspectionCopyMenu.define('inspection-copy-menu');
