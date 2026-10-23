import initialState from "../initial-state.json";
import { attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { WorkspaceElement } from "../workspace-element.js";
import type {
  ClientCertificateIdentity,
  ClientCertificatePolicy,
  ClientCertificateRule,
  ProductState,
} from "../models.js";
import { describeError } from "../utilities.js";
type RuleRow = ClientCertificateRule & {
  label: string;
  actionLabel: string;
  up: boolean;
  down: boolean;
};
type Attribute = { name: string; label: string; subject: string; issuer: string };
const attributeNames = ["CN", "O", "OU", "C", "L", "ST", "emailAddress"];
const attributeLabels = [
  "Common name",
  "Organization",
  "Organizational unit",
  "Country",
  "Locality",
  "State",
  "Email address",
];
export class ClientCertificateSettings extends WorkspaceElement {
  @attr({ mode: "boolean" }) active = false;
  @observable certificateDefault = initialState.certificateDefault;
  @observable certificateRules: RuleRow[] = [];
  @observable certificateIdentities: ClientCertificateIdentity[] = [];
  @observable certificateSelectedId = "";
  @observable certificateHost = "";
  @observable certificatePort = "443";
  @observable certificateAction = "noCertificate";
  @observable certificateSelectorMode = "identity";
  @observable certificateIdentity = "";
  @observable certificateIdentityMissing = false;
  @observable certificateAttributes: Attribute[] = initialState.certificateAttributes;
  @observable certificateSettingsBusy = false;
  @observable certificateInventoryBusy = false;
  @observable certificateSettingsText = "";
  @observable certificateSettingsError = "";
  @observable certificateInventoryError = "";
  @observable certificateSettingsDirty = false;
  @observable certificateUndoAvailable = false;
  certificatePolicyForm!: HTMLFormElement;
  certificateRuleList!: HTMLElement;
  private policy: ClientCertificatePolicy = { default: "noCertificate", rules: [] };
  private saved: ClientCertificatePolicy = structuredClone(this.policy);
  private loaded = false;
  private loadGeneration = 0;
  private inventoryGeneration = 0;
  private dragId = "";
  private undo: { rule: ClientCertificateRule; index: number } | null = null;
  activeChanged(): void {
    if (this.active && !this.loaded) {
      void this.load();
    }
  }
  protected hydratedCallback(): void {
    if (this.active && !this.loaded) {
      void this.load();
    }
  }
  async load(): Promise<void> {
    if (this.certificateSettingsBusy) {
      return;
    }
    const generation = ++this.loadGeneration;
    this.certificateSettingsBusy = true;
    this.certificateSettingsError = "";
    try {
      const state = await invoke<ProductState>("product_state");
      if (generation !== this.loadGeneration || !this.isConnected) {
        return;
      }
      this.policy = structuredClone(
        state.preferences.clientCertificatePolicy ?? { default: "noCertificate", rules: [] },
      );
      this.saved = structuredClone(this.policy);
      this.loaded = true;
      this.certificateDefault = this.policy.default;
      this.certificateSelectedId = this.policy.rules[0]?.id ?? "";
      this.render();
      this.certificateSettingsDirty = false;
      this.certificateSettingsText = "";
      this.undo = null;
      this.certificateUndoAvailable = false;
    } catch (error: unknown) {
      if (generation === this.loadGeneration) {
        this.certificateSettingsError = describeError(error);
      }
    } finally {
      if (generation === this.loadGeneration) {
        this.certificateSettingsBusy = false;
      }
    }
    if (this.loaded) {
      void this.refreshIdentities();
    }
  }
  async refreshIdentities(): Promise<void> {
    if (this.certificateInventoryBusy) {
      return;
    }
    const generation = ++this.inventoryGeneration;
    this.certificateInventoryBusy = true;
    this.certificateInventoryError = "";
    try {
      const identities = await invoke<ClientCertificateIdentity[]>("client_certificate_identities");
      if (generation === this.inventoryGeneration && this.isConnected) {
        this.certificateIdentities = identities;
        this.updateMissing();
      }
    } catch (error: unknown) {
      if (generation === this.inventoryGeneration) {
        this.certificateIdentities = [];
        this.updateMissing();
        this.certificateInventoryError = describeError(error);
      }
    } finally {
      if (generation === this.inventoryGeneration) {
        this.certificateInventoryBusy = false;
      }
    }
  }
  private updateMissing(): void {
    this.certificateIdentityMissing = Boolean(
      this.certificateIdentity &&
      !this.certificateIdentities.some((identity) => identity.id === this.certificateIdentity),
    );
  }
  private changed(): void {
    this.certificateSettingsDirty = JSON.stringify(this.policy) !== JSON.stringify(this.saved);
    this.certificateSettingsText = this.certificateSettingsDirty
      ? "Unsaved certificate settings"
      : "";
  }
  private renderRows(): void {
    this.certificateRules = this.policy.rules.map((rule, index) => ({
      ...rule,
      label: rule.host ? `${rule.host}:${rule.port}` : "New host rule",
      actionLabel:
        rule.mode.action === "automatic"
          ? "Automatic certificate"
          : rule.mode.action === "prompt"
            ? "Prompt"
            : "No certificate",
      up: index > 0,
      down: index < this.policy.rules.length - 1,
    }));
  }
  private render(): void {
    this.renderRows();
    const rule = this.policy.rules.find((rule) => rule.id === this.certificateSelectedId);
    this.certificateHost = rule?.host ?? "";
    this.certificatePort = String(rule?.port ?? 443);
    this.certificateAction = rule?.mode.action ?? "noCertificate";
    const selector = rule?.mode.action === "automatic" ? rule.mode.selector : null;
    this.certificateSelectorMode = selector && !selector.identity ? "attributes" : "identity";
    this.certificateIdentity = selector?.identity ?? "";
    this.updateMissing();
    this.certificateAttributes = attributeNames.map((name, index) => ({
      name,
      label: attributeLabels[index]!,
      subject: selector?.subject[name] ?? "",
      issuer: selector?.issuer[name] ?? "",
    }));
    this.$flushUpdates();
    this.populateInputs();
  }
  private populateInputs(): void {
    const set = (name: string, value: string) => {
      const control = this.certificatePolicyForm?.elements.namedItem(name);
      if (control instanceof HTMLInputElement || control instanceof HTMLSelectElement) {
        control.value = value;
      }
    };
    for (const [name, value] of [
      ["default", this.certificateDefault],
      ["host", this.certificateHost],
      ["port", this.certificatePort],
      ["action", this.certificateAction],
      ["selector", this.certificateSelectorMode],
      ["identity", this.certificateIdentity],
    ]) {
      set(name!, value!);
    }
    for (const attribute of this.certificateAttributes) {
      set(attribute.name + "-subject", attribute.subject);
      set(attribute.name + "-issuer", attribute.issuer);
    }
  }
  chooseRule(id: string): void {
    this.certificateSelectedId = id;
    this.render();
  }
  addRule(): void {
    if (this.policy.rules.length >= 256) {
      return;
    }
    const id = crypto.randomUUID();
    this.policy.rules.push({
      id,
      enabled: true,
      host: "",
      port: 443,
      mode: { action: "noCertificate" },
    });
    this.certificateSelectedId = id;
    this.changed();
    this.render();
  }
  removeRule(): void {
    const index = this.policy.rules.findIndex((rule) => rule.id === this.certificateSelectedId);
    if (index < 0) {
      return;
    }
    this.undo = { rule: structuredClone(this.policy.rules[index]!), index };
    this.certificateUndoAvailable = true;
    this.policy.rules = this.policy.rules.filter((rule) => rule.id !== this.certificateSelectedId);
    this.certificateSelectedId = this.policy.rules[Math.max(0, index - 1)]?.id ?? "";
    this.changed();
    this.render();
  }
  undoRemove(): void {
    if (!this.undo) {
      return;
    }
    this.policy.rules.splice(
      Math.min(this.undo.index, this.policy.rules.length),
      0,
      this.undo.rule,
    );
    this.certificateSelectedId = this.undo.rule.id;
    this.undo = null;
    this.certificateUndoAvailable = false;
    this.changed();
    this.render();
  }
  setDefault(event: Event): void {
    this.policy.default = (event.target as HTMLSelectElement)
      .value as ClientCertificatePolicy["default"];
    this.certificateDefault = this.policy.default;
    this.changed();
  }
  setEnabled(id: string, event: Event): void {
    this.policy.rules = this.policy.rules.map((rule) =>
      rule.id === id ? { ...rule, enabled: (event.target as HTMLInputElement).checked } : rule,
    );
    this.changed();
    this.render();
  }
  setField(field: string, event: Event): void {
    const value = (event.target as HTMLInputElement).value;
    if (field === "host") {
      this.certificateHost = value;
    } else if (field === "port") {
      this.certificatePort = value;
    } else if (field === "action") {
      this.certificateAction = value;
    } else if (field === "selector") {
      this.certificateSelectorMode = value;
    } else if (field === "identity") {
      this.certificateIdentity = value;
    }
    this.editSelected();
  }
  setSelectorAttribute(name: string, side: string, event: Event): void {
    this.certificateAttributes = this.certificateAttributes.map((attribute) =>
      attribute.name === name
        ? { ...attribute, [side]: (event.target as HTMLInputElement).value }
        : attribute,
    );
    this.editSelected();
  }
  private editSelected(): void {
    const subject = Object.fromEntries(
      this.certificateAttributes
        .filter((attribute) => attribute.subject.trim())
        .map((attribute) => [attribute.name, attribute.subject.trim()]),
    );
    const issuer = Object.fromEntries(
      this.certificateAttributes
        .filter((attribute) => attribute.issuer.trim())
        .map((attribute) => [attribute.name, attribute.issuer.trim()]),
    );
    const mode =
      this.certificateAction === "automatic"
        ? {
            action: "automatic" as const,
            selector: {
              identity:
                this.certificateSelectorMode === "identity"
                  ? this.certificateIdentity || null
                  : null,
              subject: this.certificateSelectorMode === "attributes" ? subject : {},
              issuer: this.certificateSelectorMode === "attributes" ? issuer : {},
            },
          }
        : { action: this.certificateAction as "noCertificate" | "prompt" };
    this.policy.rules = this.policy.rules.map((rule) =>
      rule.id === this.certificateSelectedId
        ? { ...rule, host: this.certificateHost.trim(), port: Number(this.certificatePort), mode }
        : rule,
    );
    this.changed();
    this.renderRows();
    this.updateMissing();
  }
  moveRule(id: string, offset: number): void {
    const index = this.policy.rules.findIndex((rule) => rule.id === id);
    this.reorder(id, index + offset);
  }
  private reorder(id: string, to: number): void {
    const from = this.policy.rules.findIndex((rule) => rule.id === id);
    if (from < 0 || to < 0 || to >= this.policy.rules.length || from === to) {
      return;
    }
    const [rule] = this.policy.rules.splice(from, 1);
    this.policy.rules.splice(to, 0, rule!);
    this.changed();
    this.render();
  }
  dragStart(id: string, event: DragEvent): void {
    this.dragId = id;
    event.dataTransfer?.setData("application/x-transmog-certificate-rule", id);
    if (event.dataTransfer) {
      event.dataTransfer.effectAllowed = "move";
    }
  }
  dragOver(event: DragEvent): void {
    if (this.dragId) {
      event.preventDefault();
      if (event.dataTransfer) {
        event.dataTransfer.dropEffect = "move";
      }
    }
  }
  dropRule(id: string, event: DragEvent): void {
    if (!this.dragId) {
      return;
    }
    event.preventDefault();
    this.reorder(
      this.dragId,
      this.policy.rules.findIndex((rule) => rule.id === id),
    );
    this.dragId = "";
  }
  dragEnd(): void {
    this.dragId = "";
  }
  keyboard(event: KeyboardEvent): void {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      if (this.certificateSettingsDirty) {
        this.certificatePolicyForm.requestSubmit();
      }
    }
  }
  async save(event: Event): Promise<void> {
    event.preventDefault();
    if (this.certificateSettingsBusy) {
      return;
    }
    this.certificateSettingsBusy = true;
    this.certificateSettingsError = "";
    try {
      const policy = await invoke<ClientCertificatePolicy>("save_client_certificate_policy", {
        policy: structuredClone(this.policy),
      });
      this.policy = policy;
      this.saved = structuredClone(policy);
      this.certificateSettingsDirty = false;
      this.undo = null;
      this.certificateUndoAvailable = false;
      this.render();
      this.certificateSettingsText =
        "Certificate settings saved. New connections use this policy; admitted requests can finish.";
    } catch (error: unknown) {
      this.certificateSettingsError = describeError(error);
      if (
        this.isConnected &&
        typeof error === "object" &&
        error !== null &&
        "category" in error &&
        error.category === "unavailable"
      ) {
        await this.showError(
          "Certificate settings could not be saved",
          this.certificateSettingsError,
        );
      }
    } finally {
      this.certificateSettingsBusy = false;
    }
  }
  disconnectedCallback(): void {
    this.loadGeneration++;
    this.inventoryGeneration++;
    super.disconnectedCallback();
  }
  dismissErrors(): void {
    this.certificateSettingsError = "";
    this.certificateInventoryError = "";
  }
}
ClientCertificateSettings.define("client-certificate-settings");
