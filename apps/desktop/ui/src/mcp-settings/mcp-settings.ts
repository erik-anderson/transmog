import { attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { WorkspaceElement } from "../workspace-element.js";
import { describeError } from "../utilities.js";
import initial from "../initial-state.json";

type Permission =
  | "inspect"
  | "read-bodies"
  | "collect"
  | "prepare-changes"
  | "edit-shared-rules"
  | "modify"
  | "send"
  | "read-credentials";
type Scope =
  | { kind: "all" }
  | { kind: "hosts"; hosts: string[] }
  | { kind: "requests"; instanceId: string; ids: string[] };
interface Grant {
  omitSensitiveValues?: boolean;
  id: string;
  revision: number;
  name: string;
  permissions: Permission[];
  scope: Scope;
  createdAt: number;
  expiresAt: number | null;
  lastUsedAt: number | null;
  revoked: boolean;
}
interface GrantInput {
  omitSensitiveValues: boolean;
  name: string;
  permissions: Permission[];
  scope: Scope;
  expiresAt: number | null;
}
interface GrantRow extends Grant {
  permissionText: string;
  scopeText: string;
  stateText: string;
  lastUsedText: string;
  expiryText: string;
}
interface Control {
  id: string;
  grantId: string;
  expiresAt: number;
  rules: string[];
  breakpoints: boolean;
}
interface Activity {
  grantId: string;
  name: string;
  operation: string;
  success: boolean;
  at: number;
}
interface TemporaryRule {
  id: string;
  revision: number;
  name: string;
  enabled: boolean;
}
interface Status {
  enabled: boolean;
  grants: Grant[];
  instanceId: string;
  endpoint: string | null;
  cliCommand: string | null;
  cliEnvironment?: Record<string, string>;
  controls: Control[];
  temporaryRules: TemporaryRule[];
  activity: Activity[];
  error: string | null;
}
const labels: Record<Permission, string> = {
  inspect: "View request details",
  "read-bodies": "Read message bodies",
  collect: "Control collection",
  "prepare-changes": "Prepare changes",
  "edit-shared-rules": "Edit shared rules",
  modify: "Change live traffic",
  send: "Send requests",
  "read-credentials": "Read unmasked credential headers",
};
const date = (seconds: number | null): string =>
  seconds === null ? "Never" : new Date(seconds * 1000).toLocaleString();
function localDate(seconds: number): string {
  const value = new Date(seconds * 1000);
  const pad = (part: number): string => String(part).padStart(2, "0");
  return `${value.getFullYear()}-${pad(value.getMonth() + 1)}-${pad(value.getDate())}T${pad(value.getHours())}:${pad(value.getMinutes())}`;
}

export class McpSettings extends WorkspaceElement {
  @attr({ mode: "boolean" }) active = false;
  @observable mcpEnabled = initial.mcpEnabled;
  @observable mcpBusy = initial.mcpBusy;
  @observable mcpLoaded = initial.mcpLoaded;
  @observable mcpText = initial.mcpText;
  @observable mcpError = initial.mcpError;
  @observable mcpEndpoint = initial.mcpEndpoint;
  @observable mcpRows: GrantRow[] = [];
  @observable mcpControls: Array<Control & { name: string; expiryText: string }> = [];
  @observable mcpActivity: Array<Activity & { timeText: string; outcome: string }> = [];
  @observable mcpRules: TemporaryRule[] = [];
  @observable mcpEditingId = "";
  @observable mcpScopeKind = "hosts";
  @observable mcpSelectedCount = 0;
  @observable mcpDirty = false;
  @observable mcpDialogError = "";
  @observable mcpSecret = "";
  @observable mcpConfig = "";
  @observable mcpClient = "codex";
  @observable mcpAutoStart = false;
  @observable mcpConfirmText = "";
  @observable mcpConfirmLabel = "";
  @observable mcpCopyText = "";
  mcpForm!: HTMLFormElement;
  mcpDialog!: HTMLDialogElement;
  mcpKeyDialog!: HTMLDialogElement;
  mcpConfirmDialog!: HTMLDialogElement;
  mcpCreateButton!: HTMLButtonElement;
  private snapshot: Status | undefined;
  private selectionIds: string[] = [];
  private selectionInstance = "";
  private editingRevision = 0;
  private original = "";
  private sequence = 0;
  private confirmAction: (() => Promise<void>) | undefined;
  private returnFocus: HTMLElement | undefined;
  private accessUnlisten: UnlistenFn | undefined;
  private accessGeneration = 0;
  private accessRefreshPending = false;

  activeChanged(): void {
    if (this.active && this.isConnected) {
      void this.refresh();
    }
  }
  protected hydratedCallback(): void {
    const generation = ++this.accessGeneration;
    void listen("mcp-access-changed", () => {
      if (!this.active) {
        return;
      }
      if (this.mcpBusy) {
        this.accessRefreshPending = true;
      } else {
        void this.refresh();
      }
    })
      .then((unlisten) => {
        if (this.isConnected && generation === this.accessGeneration) {
          this.accessUnlisten = unlisten;
        } else {
          unlisten();
        }
      })
      .catch(() => undefined);
    if (this.active) {
      void this.refresh();
    }
  }
  disconnectedCallback(): void {
    this.accessGeneration++;
    this.accessUnlisten?.();
    this.accessUnlisten = undefined;
    this.sequence++;
    this.clearKey();
    super.disconnectedCallback();
  }
  private refreshAccessChange(): void {
    if (this.accessRefreshPending) {
      this.accessRefreshPending = false;
      if (this.active && this.isConnected) {
        void this.refresh();
      }
    }
  }

  async refresh(): Promise<void> {
    const sequence = ++this.sequence;
    try {
      const status = await invoke<Status>("mcp_status");
      if (sequence === this.sequence) {
        this.accept(status);
      }
    } catch (error) {
      if (sequence === this.sequence) {
        this.mcpError = describeError(error);
      }
    }
  }
  private accept(status: Status): void {
    this.snapshot = status;
    this.mcpEnabled = status.enabled;
    this.mcpLoaded = true;
    this.mcpEndpoint = status.endpoint ?? "Not listening";
    this.mcpError = status.error ?? "";
    this.mcpRows = status.grants.map((grant) => ({
      ...grant,
      permissionText: grant.permissions.map((permission) => labels[permission]).join(" · "),
      scopeText:
        grant.scope.kind === "all"
          ? "All traffic in the main workspace"
          : grant.scope.kind === "hosts"
            ? grant.scope.hosts.join(", ") + " · any port"
            : `${grant.scope.ids.length} selected requests${grant.scope.instanceId === status.instanceId ? "" : " · previous app run"}`,
      stateText: grant.revoked
        ? "Revoked"
        : grant.expiresAt !== null && grant.expiresAt * 1000 <= Date.now()
          ? "Expired"
          : "Allowed",
      lastUsedText: date(grant.lastUsedAt),
      expiryText: grant.expiresAt === null ? "Until revoked" : date(grant.expiresAt),
    }));
    this.mcpControls = status.controls.map((control) => ({
      ...control,
      name: status.grants.find((grant) => grant.id === control.grantId)?.name ?? "Removed key",
      expiryText: date(control.expiresAt),
    }));
    this.mcpActivity = status.activity
      .slice()
      .reverse()
      .map((activity) => ({
        ...activity,
        timeText: date(activity.at),
        outcome: activity.success ? "Completed" : "Failed",
      }));
    this.mcpRules = status.temporaryRules;
    this.mcpText = status.enabled
      ? status.endpoint
        ? "MCP is available on this computer."
        : "MCP is enabled but its listener is unavailable. Choose Retry listener."
      : "MCP is off. Saved keys can reconnect when you enable it.";
  }
  private async run(action: () => Promise<void>): Promise<void> {
    if (this.mcpBusy) {
      return;
    }
    this.sequence++;
    this.mcpBusy = true;
    this.mcpError = "";
    try {
      await action();
    } catch (error) {
      const message = describeError(error);
      await this.refresh();
      this.mcpError = message;
    } finally {
      this.mcpBusy = false;
      this.refreshAccessChange();
    }
  }
  async toggle(event: Event): Promise<void> {
    const enabled = (event.target as HTMLInputElement).checked;
    await this.run(async () => {
      this.accept(await invoke<Status>("mcp_set_enabled", { enabled }));
    });
    if (this.snapshot) {
      (event.target as HTMLInputElement).checked = this.snapshot.enabled;
    }
  }
  async retry(): Promise<void> {
    await this.run(async () => {
      this.accept(await invoke<Status>("mcp_set_enabled", { enabled: true }));
    });
  }
  private field(name: string): HTMLInputElement {
    return this.mcpForm.elements.namedItem(name) as HTMLInputElement;
  }
  private captureSelection(): void {
    this.selectionIds = [];
    this.$emit("mcp-selection-request", {
      provide: (ids: string[]) => {
        this.selectionIds = [...ids];
      },
    });
    this.mcpSelectedCount = this.selectionIds.length;
    this.selectionInstance = this.snapshot?.instanceId ?? "";
  }
  create(): void {
    this.returnFocus = this.mcpCreateButton;
    this.mcpForm.reset();
    this.mcpEditingId = "";
    this.editingRevision = 0;
    this.mcpDialogError = "";
    this.captureSelection();
    this.mcpScopeKind = this.selectionIds.length ? "requests" : "hosts";
    this.field("scope").value = this.mcpScopeKind;
    this.field("expiry").value = localDate(Math.floor(Date.now() / 1000) + 30 * 86400);
    this.mcpDirty = true;
    this.mcpDialog.showModal();
    this.field("name").focus();
  }
  edit(id: string, event: Event): void {
    const grant = this.snapshot?.grants.find((grant) => grant.id === id);
    if (!grant) {
      return;
    }
    this.returnFocus = event.currentTarget as HTMLElement;
    this.mcpForm.reset();
    this.mcpEditingId = grant.id;
    this.editingRevision = grant.revision;
    this.mcpDialogError = "";
    this.field("name").value = grant.name;
    this.field("omit-sensitive-values").checked = grant.omitSensitiveValues ?? true;
    for (const permission of Object.keys(labels) as Permission[]) {
      this.field(permission).checked = grant.permissions.includes(permission);
    }
    this.mcpScopeKind = grant.scope.kind;
    this.field("scope").value = grant.scope.kind;
    this.field("hosts").value = grant.scope.kind === "hosts" ? grant.scope.hosts.join("\n") : "";
    this.selectionIds = grant.scope.kind === "requests" ? [...grant.scope.ids] : [];
    this.selectionInstance =
      grant.scope.kind === "requests" ? grant.scope.instanceId : (this.snapshot?.instanceId ?? "");
    this.mcpSelectedCount = this.selectionIds.length;
    this.field("expiry").value = grant.expiresAt === null ? "" : localDate(grant.expiresAt);
    this.original = JSON.stringify(this.input());
    this.mcpDirty = false;
    this.mcpDialog.showModal();
  }
  useSelection(): void {
    this.captureSelection();
    this.changed();
  }
  changed(): void {
    this.mcpScopeKind = this.field("scope").value;
    try {
      this.mcpDirty = !this.mcpEditingId || JSON.stringify(this.input()) !== this.original;
    } catch {
      this.mcpDirty = true;
    }
  }
  private input(): GrantInput {
    const data = new FormData(this.mcpForm);
    const kind = String(data.get("scope"));
    const scope: Scope =
      kind === "all"
        ? { kind: "all" }
        : kind === "requests"
          ? { kind: "requests", ids: [...this.selectionIds], instanceId: this.selectionInstance }
          : {
              kind: "hosts",
              hosts: String(data.get("hosts") ?? "")
                .split(/[\s,]+/)
                .filter(Boolean),
            };
    const expiry = String(data.get("expiry") ?? "");
    return {
      omitSensitiveValues: data.has("omit-sensitive-values"),
      name: String(data.get("name") ?? ""),
      permissions: (Object.keys(labels) as Permission[]).filter((permission) =>
        data.has(permission),
      ),
      scope,
      expiresAt: expiry ? Math.floor(new Date(expiry).getTime() / 1000) : null,
    };
  }
  async save(event: Event): Promise<void> {
    event.preventDefault();
    if (this.mcpBusy || !this.mcpDirty) {
      return;
    }
    this.mcpBusy = true;
    const sequence = ++this.sequence;
    this.mcpDialogError = "";
    try {
      const input = this.input();
      if (!input.permissions.length) {
        throw new Error("Choose at least one permission.");
      }
      if (input.scope.kind === "requests" && !input.scope.ids.length) {
        throw new Error("Select requests in Traffic, then choose Use current selection.");
      }
      if (this.mcpEditingId) {
        const status = await invoke<Status>("mcp_update_key", {
          id: this.mcpEditingId,
          revision: this.editingRevision,
          input,
        });
        if (sequence !== this.sequence || !this.isConnected) {
          return;
        }
        this.accept(status);
        this.mcpDialog.close();
      } else {
        const created = await invoke<{ key: string; status: Status }>("mcp_create_key", { input });
        if (sequence !== this.sequence || !this.isConnected) {
          return;
        }
        this.accept(created.status);
        this.mcpDialog.close();
        this.showKey(created.key);
      }
    } catch (error) {
      this.mcpDialogError = describeError(error);
    } finally {
      this.mcpBusy = false;
      this.refreshAccessChange();
    }
  }
  closeEditor(): void {
    if (!this.mcpBusy) {
      this.mcpDialog.close();
    }
  }
  cancelEditor(event: Event): void {
    if (this.mcpBusy) {
      event.preventDefault();
    }
  }
  private restoreFocus(): void {
    if (this.active && this.isConnected) {
      (this.returnFocus?.isConnected ? this.returnFocus : this.mcpCreateButton)?.focus();
    }
  }
  editorClosed(): void {
    if (!this.mcpKeyDialog.open) {
      this.restoreFocus();
    }
  }
  private showKey(key: string): void {
    if (!this.active || !this.isConnected) {
      return;
    }
    this.mcpSecret = key;
    this.mcpDialogError = "";
    this.mcpCopyText = "";
    this.mcpClient = "codex";
    this.mcpAutoStart = false;
    this.configChanged();
    this.mcpKeyDialog.showModal();
  }
  configChanged(event?: Event): void {
    if (event?.target instanceof HTMLSelectElement) {
      this.mcpClient = event.target.value;
    }
    if (event?.target instanceof HTMLInputElement) {
      this.mcpAutoStart = event.target.checked;
    }
    const command = this.snapshot?.cliCommand ?? "transmog-cli.exe";
    const args = ["mcp", "serve", ...(this.mcpAutoStart ? ["--start-app"] : [])];
    const environment = { ...this.snapshot?.cliEnvironment, TRANSMOG_ACCESS_KEY: this.mcpSecret };
    this.mcpConfig =
      this.mcpClient === "codex"
        ? `[mcp_servers.transmog]\ncommand = ${JSON.stringify(command)}\nargs = ${JSON.stringify(args)}\n\n[mcp_servers.transmog.env]\n${Object.entries(
            environment,
          )
            .map(([name, value]) => `${name} = ${JSON.stringify(value)}`)
            .join("\n")}\n`
        : JSON.stringify(
            {
              mcpServers: {
                transmog: {
                  type: "stdio",
                  command,
                  args,
                  env: environment,
                  tools: ["*"],
                },
              },
            },
            null,
            2,
          );
  }
  async copyKey(): Promise<void> {
    try {
      await navigator.clipboard.writeText(this.mcpSecret);
      this.mcpCopyText = "Access key copied.";
    } catch (error) {
      this.mcpDialogError = describeError(error);
    }
  }
  async copyConfig(): Promise<void> {
    try {
      await navigator.clipboard.writeText(this.mcpConfig);
      this.mcpCopyText = "Configuration copied. It contains the access key.";
    } catch (error) {
      this.mcpDialogError = describeError(error);
    }
  }
  closeKey(): void {
    this.mcpKeyDialog.close();
  }
  clearKey(): void {
    // A queued close event from an earlier opening must not clear a reopened dialog.
    if (this.mcpKeyDialog?.open && this.isConnected) {
      return;
    }
    this.mcpSecret = "";
    this.mcpConfig = "";
    this.mcpDialogError = "";
    this.mcpCopyText = "";
    this.restoreFocus();
  }
  private confirm(label: string, text: string, event: Event, action: () => Promise<void>): void {
    this.returnFocus = event.currentTarget as HTMLElement;
    this.mcpConfirmLabel = label;
    this.mcpConfirmText = text;
    this.confirmAction = action;
    this.mcpConfirmDialog.showModal();
  }
  revoke(id: string, event: Event): void {
    const grant = this.snapshot?.grants.find((grant) => grant.id === id);
    if (!grant) {
      return;
    }
    this.confirm(
      "Revoke access",
      `Revoke “${grant.name}”? Its key will stop working and its temporary traffic changes will end.`,
      event,
      async () => {
        this.accept(await invoke<Status>("mcp_revoke_key", { id, revision: grant.revision }));
      },
    );
  }
  revokeAll(event: Event): void {
    this.confirm(
      "Revoke all access",
      "Permanently invalidate every access key and end all agent control sessions?",
      event,
      async () => {
        this.accept(await invoke<Status>("mcp_revoke_key", { id: null, revision: null }));
      },
    );
  }
  replace(id: string, event: Event): void {
    const grant = this.snapshot?.grants.find((grant) => grant.id === id);
    if (!grant) {
      return;
    }
    this.confirm(
      "Replace key",
      `Replace the key for “${grant.name}”? The old key will immediately stop working. Update the agent's configuration with the new key.`,
      event,
      async () => {
        const created = await invoke<{ key: string; status: Status }>("mcp_replace_key", {
          id,
          revision: grant.revision,
        });
        this.accept(created.status);
        this.showKey(created.key);
      },
    );
  }
  async confirmNow(): Promise<void> {
    const action = this.confirmAction;
    if (!action || this.mcpBusy) {
      return;
    }
    this.mcpConfirmDialog.close();
    await this.run(action);
    if (!this.mcpKeyDialog.open) {
      this.restoreFocus();
    }
  }
  closeConfirm(): void {
    this.mcpConfirmDialog.close();
  }
  confirmClosed(): void {
    this.confirmAction = undefined;
    if (!this.mcpKeyDialog.open) {
      this.restoreFocus();
    }
  }
  async removeRecord(id: string): Promise<void> {
    const grant = this.snapshot?.grants.find((grant) => grant.id === id);
    if (grant) {
      await this.run(async () => {
        this.accept(await invoke<Status>("mcp_remove_key", { id, revision: grant.revision }));
      });
    }
  }
  async endControl(id: string): Promise<void> {
    await this.run(async () => {
      this.accept(await invoke<Status>("mcp_end_control", { id }));
    });
  }
  reviewRule(id: string): void {
    this.$emit("matched-rule-request", id);
  }
  keepRule(id: string, event: Event): void {
    const rule = this.mcpRules.find((rule) => rule.id === id);
    if (rule) {
      this.confirm(
        "Keep changes",
        "Keep this reviewed rule as ordinary saved configuration? It will no longer end with the agent's control session.",
        event,
        async () => {
          this.accept(await invoke<Status>("mcp_keep_rule", { id, revision: rule.revision }));
        },
      );
    }
  }
}
McpSettings.define("mcp-settings");
