import { attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { WorkspaceElement } from "../workspace-element.js";
import { describeError } from "../utilities.js";
import initial from "../initial-state.json";

interface Approval {
  id: string;
  grantId: string;
  revision: number;
  name: string;
  reason: string;
  state: "pending" | "temporary";
  remainingSeconds: number;
  scope: { kind: "all" | "hosts" | "requests"; hosts?: string[]; ids?: string[] };
}
interface Summary {
  enabled: boolean;
  controls: number;
  sensitiveRequests?: Approval[];
}
interface ApprovalRow extends Approval {
  scopeText: string;
  remainingText: string;
}
const countdown = (seconds: number): string =>
  `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;

export class McpIndicator extends WorkspaceElement {
  @attr({ mode: "boolean" }) viewerMode = false;
  @observable mcpIndicatorVisible = initial.mcpIndicatorVisible;
  @observable mcpIndicatorText = initial.mcpIndicatorText;
  @observable mcpSensitiveRequests: ApprovalRow[] = [];
  @observable mcpSensitiveTemporary: ApprovalRow[] = [];
  @observable mcpSensitiveError = initial.mcpSensitiveError;
  @observable mcpSensitiveBusy = initial.mcpSensitiveBusy;
  mcpAccessPanel!: HTMLElement;
  mcpIndicatorButton!: HTMLButtonElement;
  private restoreOnClose = false;
  private readonly markEscape = (event: KeyboardEvent): void => {
    if (event.key === "Escape" && this.mcpAccessPanel?.matches(":popover-open")) {
      this.restoreOnClose = true;
    }
  };
  private generation = 0;
  private sequence = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private unlisten: UnlistenFn | undefined;

  protected hydratedCallback(): void {
    window.addEventListener("keydown", this.markEscape, true);
    const generation = ++this.generation;
    void this.poll(generation);
    if (!this.viewerMode) {
      void listen("mcp-access-changed", () => void this.refresh())
        .then((unlisten) => {
          if (this.isConnected && generation === this.generation) {
            this.unlisten = unlisten;
          } else {
            unlisten();
          }
        })
        .catch(() => undefined);
    }
  }
  private accept(summary: Summary): void {
    const rows = (summary.sensitiveRequests ?? []).map((entry) => ({
      ...entry,
      scopeText:
        entry.scope.kind === "all"
          ? "All traffic in the main workspace"
          : entry.scope.kind === "hosts"
            ? (entry.scope.hosts ?? []).join(", ") + " · any port"
            : `${entry.scope.ids?.length ?? 0} selected requests`,
      remainingText: countdown(Math.max(0, entry.remainingSeconds)),
    }));
    this.mcpSensitiveRequests = rows.filter((entry) => entry.state === "pending");
    this.mcpSensitiveTemporary = rows.filter((entry) => entry.state === "temporary");
    this.mcpIndicatorVisible = summary.enabled || rows.length > 0;
    this.mcpIndicatorText = this.mcpSensitiveTemporary.length
      ? `MCP · original values ${countdown(Math.min(...this.mcpSensitiveTemporary.map((entry) => entry.remainingSeconds)))}`
      : this.mcpSensitiveRequests.length
        ? `MCP · ${this.mcpSensitiveRequests.length} access request${this.mcpSensitiveRequests.length === 1 ? "" : "s"}`
        : summary.controls
          ? `MCP · ${summary.controls} control session${summary.controls === 1 ? "" : "s"}`
          : "MCP enabled";
  }
  async refresh(): Promise<void> {
    if (this.viewerMode || !this.isConnected) {
      return;
    }
    const sequence = ++this.sequence;
    try {
      const summary = await invoke<Summary>("mcp_summary");
      if (sequence === this.sequence && this.isConnected) {
        this.accept(summary);
      }
    } catch {
      /* Preserve the last known active-access indicator during a status failure. */
    }
  }
  private async poll(generation: number): Promise<void> {
    if (generation !== this.generation || !this.isConnected || this.viewerMode) {
      return;
    }
    await this.refresh();
    if (generation === this.generation) {
      this.timer = setTimeout(() => void this.poll(generation), 1000);
    }
  }
  private async change(command: string, args: Record<string, string>): Promise<void> {
    if (this.mcpSensitiveBusy) {
      return;
    }
    this.mcpSensitiveBusy = true;
    this.mcpSensitiveError = "";
    this.sequence++;
    try {
      const summary = await invoke<Summary>(command, args);
      if (this.isConnected) {
        this.sequence++;
        this.accept(summary);
      }
    } catch (error) {
      await this.refresh();
      this.mcpSensitiveError = describeError(error);
    } finally {
      this.mcpSensitiveBusy = false;
    }
  }
  async decide(id: string, choice: string): Promise<void> {
    await this.change("mcp_sensitive_decide", { id, choice });
  }
  async revoke(id: string): Promise<void> {
    await this.change("mcp_sensitive_revoke", { id });
  }
  open(): void {
    this.mcpAccessPanel.hidePopover();
    this.$emit("mcp-settings-request");
  }
  prepareClose(): void {
    this.restoreOnClose = true;
  }
  panelToggled(event: Event): void {
    if ((event as ToggleEvent).newState === "closed") {
      if (
        this.restoreOnClose &&
        this.mcpIndicatorButton.isConnected &&
        !this.mcpIndicatorButton.hidden
      ) {
        this.mcpIndicatorButton.focus();
      }
    }
    this.restoreOnClose = false;
  }
  disconnectedCallback(): void {
    window.removeEventListener("keydown", this.markEscape, true);
    this.generation++;
    this.sequence++;
    clearTimeout(this.timer);
    this.unlisten?.();
    this.unlisten = undefined;
    super.disconnectedCallback();
  }
}
McpIndicator.define("mcp-indicator");
