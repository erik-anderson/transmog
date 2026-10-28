import { attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { WorkspaceElement } from "../workspace-element.js";
import initial from "../initial-state.json";

export class McpIndicator extends WorkspaceElement {
  @attr({ mode: "boolean" }) viewerMode = false;
  @observable mcpIndicatorVisible = initial.mcpIndicatorVisible;
  @observable mcpIndicatorText = initial.mcpIndicatorText;
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  protected hydratedCallback(): void {
    void this.poll(++this.generation);
  }
  private async poll(generation: number): Promise<void> {
    if (this.viewerMode || generation !== this.generation || !this.isConnected) {
      return;
    }
    try {
      const summary = await invoke<{ enabled: boolean; controls: number }>("mcp_summary");
      if (generation !== this.generation) {
        return;
      }
      this.mcpIndicatorVisible = summary.enabled;
      this.mcpIndicatorText = summary.controls
        ? `MCP · ${summary.controls} control session${summary.controls === 1 ? "" : "s"}`
        : "MCP enabled";
    } catch {
      /* Keep the last known enabled state until the next status read. */
    }
    if (generation === this.generation) {
      this.timer = setTimeout(() => void this.poll(generation), 3000);
    }
  }
  open(): void {
    this.$emit("mcp-settings-request");
  }
  disconnectedCallback(): void {
    this.generation++;
    clearTimeout(this.timer);
    super.disconnectedCallback();
  }
}
McpIndicator.define("mcp-indicator");
