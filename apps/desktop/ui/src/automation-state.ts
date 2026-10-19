import type {AutomationRule, AutomationStatus} from './models.js';

/** Owned content stays in Response sets, including applied and named snapshots. */
export function experimentRuleIds(status: AutomationStatus | null): Set<string> {
  return new Set(status?.responseSets?.flatMap(set => [...set.rules,...(set.applied?.rules??[]),...set.versions.flatMap(version=>version.rules)].map(rule=>rule.id)) ?? []);
}

/** Saved responses that can participate in new traffic under the current gates. */
export function activeSavedResponseCount(status: AutomationStatus | null): number {
  if (!status?.autoresponsesEnabled) return 0;
  const paused = new Set(status.responseSets?.filter(set=>set.paused).flatMap(set=>set.applied?.rules.map(rule=>rule.id)??[])??[]);
  return status.rules.filter(rule=>rule.enabled!==false && rule.request.responseAsset && !paused.has(rule.id)).length;
}

/** Counts configured participation, independent of filters, matches and proxy lifecycle. */
export function automationActivity(status: AutomationStatus | null, scripts: number) {
  const owned = experimentRuleIds(status);
  const enabled = (status?.rules ?? []).filter(rule => rule.enabled !== false && !owned.has(rule.id));
  const pausedRules = enabled.filter(rule => rule.request.responseAsset && status?.autoresponsesEnabled === false).length;
  const rules = enabled.length - pausedRules;
  let experiments = 0, pausedExperiments = 0;
  for (const set of status?.responseSets ?? []) {
    if (!set.applied?.rules.some(rule => rule.enabled !== false)) continue;
    if (set.paused || status?.autoresponsesEnabled === false) pausedExperiments++;
    else experiments++;
  }
  const total = rules + experiments + scripts;
  const paused = pausedRules + pausedExperiments;
  const count=(value:number,noun:string)=>`${value} ${noun}${value===1?'':'s'}`;
  const summary=(total?'Enabled for new traffic: '+[rules?count(rules,'rule'):'',experiments?count(experiments,'response set'):'',scripts?count(scripts,'script'):''].filter(Boolean).join(' · '):'No active automation')+(paused?` · ${paused} paused`:'');
  const label = `${count(rules,'active rule')} · ${count(experiments,'active response set')} · ${count(scripts,'active script')}` +
    (paused ? ` · ${paused} paused` : '') + '. Active means enabled and unpaused for new traffic; the proxy must be running.';
  return {rules, experiments, scripts, total, paused, label, summary};
}

/** Activation-only updates can advance an editor's revision without replacing its draft. */
export function sameRuleContent(left: AutomationRule, right: AutomationRule): boolean {
  const content = ({enabled: _enabled, revision: _revision, ...rule}: AutomationRule) => rule;
  return JSON.stringify(content(left)) === JSON.stringify(content(right));
}

export interface RuleEnabledRequest {id: string; revision: number; enabled: boolean;}
