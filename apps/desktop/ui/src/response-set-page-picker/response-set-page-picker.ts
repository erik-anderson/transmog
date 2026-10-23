import { WebUIElement, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import type {
  AutomationStatus,
  ExperimentPreviewPage,
  ExperimentPreviewPages,
  PreviewPageTarget,
} from "../models.js";
import { describeError } from "../utilities.js";

export function pageIdentity(page: ExperimentPreviewPage): string {
  const date = new Date(page.startedAt);
  return `Request #${page.requestNumber} · ${page.method} · ${page.source} · ${Number.isFinite(date.valueOf()) ? date.toISOString() : "Recorded time unavailable"}`;
}
export class ResponseSetPagePicker extends WebUIElement {
  @observable pages: Array<ExperimentPreviewPage & { identity: string }> = [];
  @observable selectedId = "";
  @observable rememberedUrl = "";
  @observable available = false;
  @observable more = false;
  @observable saving = false;
  @observable pageSaved = false;
  @observable error = "";
  pageDialog!: HTMLDialogElement;
  private epoch = 0;
  private setId = "";
  private revision = 0;
  private current: () => boolean = () => false;
  private returnFocus: HTMLElement | null = null;
  private resolve: ((target: PreviewPageTarget | null) => void) | null = null;
  async request(
    id: string,
    selectedId: string,
    preview: boolean,
    current: () => boolean,
  ): Promise<PreviewPageTarget | null> {
    this.cancel();
    const epoch = ++this.epoch;
    const active = (this.getRootNode() as ShadowRoot).activeElement;
    let result: ExperimentPreviewPages;
    let canceled = false;
    const root = this.getRootNode(),
      cancel = (event: Event) => {
        if (event instanceof KeyboardEvent && event.key === "Escape") {
          canceled = true;
        }
      };
    root.addEventListener("keydown", cancel, true);
    try {
      result = await invoke<ExperimentPreviewPages>("experiment_preview_pages", {
        id,
        selectedId: selectedId || null,
      });
    } catch (error) {
      if (!canceled && epoch === this.epoch && current()) {
        this.$emit("notice", {
          title: "Captured pages unavailable",
          message: describeError(error),
          actionLabel: null,
          action: null,
        });
      }
      return null;
    } finally {
      root.removeEventListener("keydown", cancel, true);
    }
    if (
      canceled ||
      epoch !== this.epoch ||
      !current() ||
      (this.getRootNode() as ShadowRoot).activeElement !== active
    ) {
      return null;
    }
    if (preview && result.page && result.available) {
      return { page: result.page, sourceLabel: result.sourceLabel };
    }
    this.setId = id;
    this.revision = result.revision;
    this.current = current;
    this.returnFocus = active instanceof HTMLElement ? active : null;
    this.pages = result.pages.map((page) => ({ ...page, identity: pageIdentity(page) }));
    this.selectedId = "";
    this.rememberedUrl = result.page?.url ?? "";
    this.available = result.available;
    this.more = result.more;
    this.error = "";
    this.pageSaved = false;
    this.$flushUpdates();
    this.pageDialog.showModal();
    return new Promise((resolve) => {
      this.resolve = resolve;
    });
  }
  select(id: string): void {
    if (!this.saving && !this.pageSaved) {
      this.selectedId = id;
    }
  }
  usePointerDown(event: PointerEvent): void {
    if (this.pageSaved) {
      event.preventDefault();
    }
  }
  async usePage(): Promise<void> {
    const page = this.pages.find((page) => page.entryId === this.selectedId);
    if (!page || this.saving || this.pageSaved || !this.current()) {
      return;
    }
    const { identity: _, ...reference } = page;
    const epoch = this.epoch,
      id = this.setId;
    const focused = (this.getRootNode() as ShadowRoot).activeElement;
    this.saving = true;
    this.error = "";
    try {
      const status = await invoke<AutomationStatus>("set_experiment_preview_page", {
        id,
        revision: this.revision,
        page: reference,
      });
      this.$emit("preview-page-status", status);
      if (epoch !== this.epoch || !this.pageDialog.open || !this.current()) {
        return;
      }
      if ((this.getRootNode() as ShadowRoot).activeElement !== focused) {
        this.rememberedUrl = reference.url;
        this.available = true;
        this.pageSaved = true;
        return;
      }
      this.finish({ page: reference, sourceLabel: null });
    } catch (error) {
      const message = describeError(error);
      if (epoch === this.epoch && this.pageDialog.open) {
        this.error = message;
      } else {
        this.$emit("notice", {
          title: "Page association failed",
          message,
          actionLabel: null,
          action: null,
        });
      }
    } finally {
      this.saving = false;
    }
  }
  private finish(target: PreviewPageTarget | null): void {
    const resolve = this.resolve;
    this.resolve = null;
    this.epoch++;
    this.pageDialog?.close();
    const focus = this.returnFocus;
    this.returnFocus = null;
    if (focus?.isConnected && focus.getClientRects().length) {
      focus.focus({ preventScroll: true });
    }
    resolve?.(target);
  }
  cancel(event?: Event): void {
    event?.preventDefault();
    this.finish(null);
  }
  closed(): void {
    if (!this.pageDialog.open) {
      this.finish(null);
    }
  }
  goToTraffic(): void {
    this.cancel();
    this.$emit("page-traffic-request", undefined);
  }
  disconnectedCallback(): void {
    this.cancel();
    super.disconnectedCallback();
  }
}
ResponseSetPagePicker.define("response-set-page-picker");
