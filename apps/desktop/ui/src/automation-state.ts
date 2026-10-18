import type {AutomationRule, AutomationStatus} from './models.js';

/** Experiment-owned rules belong to Experiments, including inactive revisions. */
export function experimentRuleIds(status: AutomationStatus | null): Set<string> {
  return new Set(status?.experiments?.flatMap(experiment => experiment.variants.flatMap(variant =>
    variant.revisions.flatMap(revision => revision.rules.map(rule => rule.id)))) ?? []);
}

/** Counts configured participation, independent of filters, matches and proxy lifecycle. */
export function automationActivity(status: AutomationStatus | null, scripts: number) {
  const owned = experimentRuleIds(status);
  const enabled = (status?.rules ?? []).filter(rule => rule.enabled !== false && !owned.has(rule.id));
  const pausedRules = enabled.filter(rule => rule.request.responseAsset && status?.autoresponsesEnabled === false).length;
  const rules = enabled.length - pausedRules;
  let experiments = 0, pausedExperiments = 0;
  for (const experiment of status?.experiments ?? []) {
    if (experiment.archived || !experiment.active || experiment.active.variantId === 'baseline') continue;
    const variant = experiment.variants.find(item => item.id === experiment.active!.variantId);
    const revision = variant?.revisions.find(item => item.revision === experiment.active!.revision);
    if (!revision?.rules.some(rule => rule.enabled !== false)) continue;
    if (experiment.paused || status?.autoresponsesEnabled === false) pausedExperiments++;
    else experiments++;
  }
  const total = rules + experiments + scripts;
  const paused = pausedRules + pausedExperiments;
  const count=(value:number,noun:string)=>`${value} ${noun}${value===1?'':'s'}`;
  const summary=(total?'Enabled for new traffic: '+[rules?count(rules,'rule'):'',experiments?count(experiments,'experiment'):'',scripts?count(scripts,'script'):''].filter(Boolean).join(' · '):'No active automation')+(paused?` · ${paused} paused`:'');
  const label = `${count(rules,'active rule')} · ${count(experiments,'active experiment')} · ${count(scripts,'active script')}` +
    (paused ? ` · ${paused} paused` : '') + '. Active means enabled and unpaused for new traffic; the proxy must be running.';
  return {rules, experiments, scripts, total, paused, label, summary};
}

/** Activation-only updates can advance an editor's revision without replacing its draft. */
export function sameRuleContent(left: AutomationRule, right: AutomationRule): boolean {
  const content = ({enabled: _enabled, revision: _revision, ...rule}: AutomationRule) => rule;
  return JSON.stringify(content(left)) === JSON.stringify(content(right));
}

export interface RuleEnabledRequest {id: string; revision: number; enabled: boolean;}
