import {WebUIElement, observable} from '@microsoft/webui-framework';
import type {AutomationRule} from '../models.js';
import type {RuleEnabledRequest} from '../automation-state.js';

/** A saved-rule action. It never submits, saves or replaces editor content. */
export class RuleActivation extends WebUIElement {
  @observable rule: AutomationRule | null = null;
  @observable busy = false;
  @observable activationPaused = false;
  @observable activationLabel = '';
  @observable activationPressed = 'false';
  @observable activationEnabled = false;
  ruleChanged(): void { this.activationEnabled = !!this.rule && this.rule.enabled !== false; this.activationPressed = String(this.activationEnabled); this.activationLabel = `${this.rule?.enabled === false ? 'Enable' : 'Disable'} ${this.rule?.displayName ?? 'rule'}`; }
  toggle(event: Event): void {
    event.stopPropagation();
    if (!this.rule || this.busy) return;
    this.$emit('rule-enabled-request', {id:this.rule.id, revision:this.rule.revision, enabled:this.rule.enabled === false} satisfies RuleEnabledRequest);
  }
  stop(event: Event): void { event.stopPropagation(); }
}
RuleActivation.define('rule-activation');
