import { WebUIElement, observable } from "@microsoft/webui-framework";
import { renderPresentation } from "../rendering.js";
import {
  boundedCopy,
  CopyError,
  copyDialog,
  copyGesture,
  requestCopy,
  type CopyRequest,
} from "../inspection-copy.js";
import { surfaceCopy } from "../inspection-copy-surface.js";
import { requestFormatConverter, selectedConverterText } from "../converter-selection.js";
import { MeasuredMenu, menuAnchor, navigateMenu } from "../context-menu.js";

export class InspectionCopyMenu extends WebUIElement {
  @observable copyMenuLabel = "Copy inspection value";
  @observable copyActions: Array<{ id: string; label: string; disabled: boolean }> = [];
  @observable copyMenuX = "8px";
  @observable copyMenuY = "8px";
  @observable copyMenuNote = "";
  @observable copyMenuStatus = "";
  @observable copyMenuBusy = false;
  @observable copyFeedback = "";
  @observable copyColorScheme = "light dark";
  menu!: HTMLDivElement;
  feedback!: HTMLElement;
  private feedbackTimer: ReturnType<typeof setTimeout> | undefined;
  private request: CopyRequest | null = null;
  private revision = 0;
  private feedbackRevision = 0;
  private readonly placement = new MeasuredMenu(
    () => this.menu,
    (x, y) => {
      this.copyMenuX = x;
      this.copyMenuY = y;
    },
  );
  private readonly receive = async (event: Event): Promise<void> => {
    const request = (event as CustomEvent<CopyRequest>).detail;
    if (copyDialog(request.source) !== copyDialog(this)) {
      return;
    }
    this.close(false);
    this.request = request;
    this.copyMenuLabel = request.label ?? "Copy inspection value";
    this.hideFeedback();
    this.copyColorScheme = getComputedStyle(request.source).colorScheme;
    this.copyActions = request.actions.map((action) => ({
      id: action.id,
      label: action.label,
      disabled: !!action.disabled,
    }));
    this.copyMenuNote = request.note ?? "";
    this.copyMenuStatus = "";
    const revision = this.revision;
    const current = await renderPresentation(this, () => [this.revision, this.request]);
    if (
      !current() ||
      revision !== this.revision ||
      this.request !== request ||
      !this.isConnected ||
      document.hidden ||
      !request.source.isConnected ||
      !request.current()
    ) {
      return;
    }
    const anchor = request.anchor ?? menuAnchor(request.event, request.source);
    this.menu.showPopover({ source: request.source });
    this.placement.open(anchor);
    this.buttons()[0]?.focus();
  };
  private hideFeedback(): void {
    this.feedbackRevision++;
    clearTimeout(this.feedbackTimer);
    this.copyFeedback = "";
    if (this.feedback?.matches(":popover-open")) {
      this.feedback.hidePopover();
    }
  }
  private async showFeedback(): Promise<void> {
    this.hideFeedback();
    const revision = this.feedbackRevision,
      dialog = copyDialog(this);
    this.copyFeedback = "Copied.";
    await this.$whenRendered();
    if (
      !this.isConnected ||
      document.hidden ||
      revision !== this.feedbackRevision ||
      (dialog && !dialog.open)
    ) {
      return;
    }
    this.feedback.showPopover();
    this.feedbackTimer = setTimeout(() => this.hideFeedback(), 1800);
  }
  private readonly copied = async (event: Event): Promise<void> => {
    if (
      copyDialog((event as CustomEvent<{ source: HTMLElement }>).detail.source) === copyDialog(this)
    ) {
      await this.showFeedback();
    }
  };
  protected hydratedCallback(): void {
    document.addEventListener("inspection-copied", this.copied);
    document.addEventListener("inspection-copy-request", this.receive);
    document.addEventListener("visibilitychange", this.visibility);
    document.addEventListener("contextmenu", this.context);
    document.addEventListener("keydown", this.context);
  }
  private readonly context = (event: MouseEvent | KeyboardEvent): void => {
    if (!copyGesture(event)) {
      return;
    }
    // Only the menu in the originating modal builds the section snapshot.
    const target = event
      .composedPath()
      .find((item): item is HTMLElement => item instanceof HTMLElement);
    if (!target || copyDialog(target) !== copyDialog(this)) {
      return;
    }
    const selection = selectedConverterText(event);
    if (
      selection &&
      copyDialog(target) &&
      !target.isContentEditable &&
      !target.matches("input,textarea:not([readonly])")
    ) {
      const current = (): boolean =>
        selection.source.isConnected &&
        (selection.source instanceof HTMLTextAreaElement ||
        selection.source instanceof HTMLInputElement
          ? selection.source.value === selection.inputValue
          : !selection.range || selection.range.toString() === selection.text);
      requestCopy(event, {
        source: selection.source,
        current,
        actions: [{ id: "selection", label: "Copy", read: () => selection.text }],
      });
      return;
    }
    const request = surfaceCopy(event);
    if (request && copyDialog(request.source) === copyDialog(this)) {
      requestCopy(event, request);
    }
  };
  private readonly visibility = (): void => {
    if (document.hidden) {
      this.close(false);
      this.hideFeedback();
    }
  };
  private buttons(): HTMLButtonElement[] {
    return [...this.menu.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
  }
  async execute(id: string): Promise<void> {
    const request = this.request,
      action = request?.actions.find((action) => action.id === id);
    if (!request || !action || action.disabled || this.copyMenuBusy) {
      return;
    }
    const revision = this.revision;
    const current = (): boolean =>
      revision === this.revision &&
      this.request === request &&
      this.isConnected &&
      !document.hidden &&
      request.source.isConnected &&
      request.source.getClientRects().length > 0 &&
      request.current() &&
      this.menu.matches(":popover-open");
    this.copyMenuBusy = true;
    this.copyMenuStatus = action.convert ? "Preparing converter input…" : "Preparing copy…";
    try {
      if (!current()) {
        this.close(false);
        return;
      }
      if (action.run) {
        this.close();
        await action.run();
        return;
      }
      const conversion = action.readConversion ? await action.readConversion(current) : null;
      if (!conversion && !action.read) {
        throw new CopyError("This action is unavailable. Open the menu again.");
      }
      const text = boundedCopy(conversion ? conversion.text : await action.read!(current));
      if (!current()) {
        this.close(false);
        return;
      }
      if (action.convert) {
        this.close();
        requestFormatConverter(request.source, text, conversion?.operation);
        return;
      }
      await navigator.clipboard.writeText(text);
      if (current()) {
        this.close();
        await this.showFeedback();
      }
    } catch (error: unknown) {
      if (current()) {
        this.copyMenuStatus =
          error instanceof CopyError
            ? error.message
            : action.convert
              ? "Converter input is unavailable. Inspect the current evidence again."
              : "Copy failed. The clipboard or retained value is unavailable.";
      }
    } finally {
      if (revision === this.revision) {
        this.copyMenuBusy = false;
      }
    }
  }
  close(focus = true): void {
    this.placement.close();
    const source = this.request?.source,
      restoreFocus = this.request?.restoreFocus;
    this.revision++;
    this.request = null;
    this.copyMenuBusy = false;
    if (this.menu?.matches(":popover-open")) {
      this.menu.hidePopover();
    }
    this.copyActions = [];
    this.copyMenuNote = "";
    if (focus && source?.isConnected) {
      if (restoreFocus) {
        restoreFocus();
        return;
      }
      const target = source.closest<HTMLElement>("[tabindex],button,input,textarea") ?? source;
      target.focus({ preventScroll: true });
    }
  }
  toggled(event: ToggleEvent): void {
    if (event.newState === "closed") {
      this.close(false);
    }
  }
  key(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      this.close();
      return;
    }
    navigateMenu(event, this.buttons(), this.shadowRoot?.activeElement ?? null);
  }
  disconnectedCallback(): void {
    this.hideFeedback();
    document.removeEventListener("inspection-copied", this.copied);
    this.close(false);
    document.removeEventListener("inspection-copy-request", this.receive);
    document.removeEventListener("visibilitychange", this.visibility);
    super.disconnectedCallback();
    document.removeEventListener("contextmenu", this.context);
    document.removeEventListener("keydown", this.context);
  }
}
InspectionCopyMenu.define("inspection-copy-menu");
