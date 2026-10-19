import assert from 'node:assert/strict';
import {test} from 'node:test';
import {automationActivity,activeSavedResponseCount} from '../src/automation-state.ts';

test('saved response count includes only enabled responses with both live gates open',()=>{
  const rule=(id,asset='asset@1',enabled=true)=>({id,enabled,request:{responseAsset:asset}});
  const state={autoresponsesEnabled:true,rules:[rule('ordinary'),rule('paused-set'),rule('active-set'),rule('disabled',undefined,false),rule('header',null)],responseSets:[
    {paused:true,applied:{rules:[rule('paused-set')]}},
    {paused:false,applied:{rules:[rule('active-set')]}},
  ]};
  assert.equal(activeSavedResponseCount(state),2);
  state.responseSets[0].paused=false;
  assert.equal(activeSavedResponseCount(state),3);
  state.autoresponsesEnabled=false;
  assert.equal(activeSavedResponseCount(state),0);
});

test('navigation counts ordinary rules, response sets once, and scripts; pause never changes authored enablement',()=>{
  const rule=(id,asset=null,enabled=true)=>({id,enabled,request:{responseAsset:asset}});
  const owned=rule('owned','asset@1'),ordinary=rule('saved','asset@1');
  const experiment={id:'set',paused:false,rules:[owned],versions:[],applied:{revision:1,rules:[owned,rule('owned-2','asset@2')]}};
  const state={rules:[rule('header'),ordinary,owned,rule('off',null,false)],responseSets:[experiment],autoresponsesEnabled:true};
  const counts=()=>{const {rules,experiments,scripts,total,paused}=automationActivity(state,2);return {rules,experiments,scripts,total,paused};};
  assert.deepEqual(counts(),{rules:2,experiments:1,scripts:2,total:5,paused:0});
  state.autoresponsesEnabled=false;
  assert.deepEqual(counts(),{rules:1,experiments:0,scripts:2,total:3,paused:2});
  assert.equal(ordinary.enabled,true);
  state.autoresponsesEnabled=true;experiment.paused=true;
  assert.deepEqual(counts(),{rules:2,experiments:0,scripts:2,total:4,paused:1});
  experiment.applied=null;
  assert.deepEqual(counts(),{rules:2,experiments:0,scripts:2,total:4,paused:0});
});
