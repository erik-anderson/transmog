import assert from 'node:assert/strict';
import {test} from 'node:test';
import {automationActivity} from '../src/automation-state.ts';

test('navigation counts ordinary rules, experiments once, and scripts; pause never changes authored enablement',()=>{
  const rule=(id,asset=null,enabled=true)=>({id,enabled,request:{responseAsset:asset}});
  const owned=rule('owned','asset@1'),ordinary=rule('saved','asset@1');
  const experiment={id:'experiment',archived:false,paused:false,active:{variantId:'edited',revision:1},variants:[{id:'edited',revisions:[{revision:1,rules:[owned,rule('owned-2','asset@2')]}]}]};
  const state={rules:[rule('header'),ordinary,owned,rule('off',null,false)],experiments:[experiment],autoresponsesEnabled:true};
  const counts=()=>{const {rules,experiments,scripts,total,paused}=automationActivity(state,2);return {rules,experiments,scripts,total,paused};};
  assert.deepEqual(counts(),{rules:2,experiments:1,scripts:2,total:5,paused:0});
  state.autoresponsesEnabled=false;
  assert.deepEqual(counts(),{rules:1,experiments:0,scripts:2,total:3,paused:2});
  assert.equal(ordinary.enabled,true);
  state.autoresponsesEnabled=true;experiment.paused=true;
  assert.deepEqual(counts(),{rules:2,experiments:0,scripts:2,total:4,paused:1});
  experiment.active={variantId:'baseline',revision:1};
  assert.deepEqual(counts(),{rules:2,experiments:0,scripts:2,total:4,paused:0});
});
