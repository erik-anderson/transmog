import assert from 'node:assert/strict';

// Use the native viewer's retained SAZ body; restore the temporary connection
// facts immediately so the rest of the native flow reads original evidence.
export async function smokeConnectTunnels(viewer) {
  const original=await viewer.evaluate(`(()=>{
    const workspace=document.querySelector('app-shell').traffic;
    const inspector=workspace.querySelector('message-inspector[side="response"]');
    return {detail:structuredClone(workspace.selectedDetail),mode:inspector.mode,viewer:inspector.viewer};
  })()`);
  try {
    const decrypted=await viewer.evaluate(`(async()=>{
      const workspace=document.querySelector('app-shell').traffic,detail=structuredClone(workspace.selectedDetail);
      detail.performance={points:[],protocols:[],transports:[{leg:'client',connectionId:'native-connect',tunnelDestination:'media.stores.com:443',tlsVersion:'TLSv1.3'}]};
      workspace.applyDetail(detail);
      const inspector=workspace.querySelector('message-inspector[side="response"]');
      inspector.viewer='auto';inspector.showMode('body');await inspector.inspectBody();inspector.$flushUpdates();
      return {text:inspector.bodyText,notice:inspector.bodyNotice,rendered:inspector.textContent};
    })()`);
    assert.equal(decrypted.text,'saved viewer response');
    assert.equal(decrypted.notice,'');
    assert.match(decrypted.rendered,/saved viewer response/);
    assert.doesNotMatch(decrypted.rendered,/HTTPS decryption was disabled/);
    const opaque=await viewer.evaluate(`(async()=>{
      const workspace=document.querySelector('app-shell').traffic,detail=structuredClone(workspace.selectedDetail);
      detail.requests[0].method='CONNECT';
      detail.performance.transports=[{leg:'upstream',connectionId:'native-opaque',tunnelDestination:'media.stores.com:443'}];
      workspace.applyDetail(detail);
      const inspector=workspace.querySelector('message-inspector[side="response"]');
      await inspector.inspectBody();inspector.$flushUpdates();
      return {text:inspector.bodyText,notice:inspector.bodyNotice,rendered:inspector.textContent};
    })()`);
    assert.equal(opaque.text,'');
    assert.match(opaque.notice,/Opaque CONNECT tunnel to media.stores.com:443/);
    assert.match(opaque.rendered,/Opaque CONNECT tunnel to media.stores.com:443/);
    assert.doesNotMatch(opaque.rendered,/saved viewer response/);
  } finally {
    await viewer.evaluate(`(async()=>{
      const original=${JSON.stringify(original)},workspace=document.querySelector('app-shell').traffic;
      workspace.applyDetail(original.detail);
      const inspector=workspace.querySelector('message-inspector[side="response"]');
      inspector.viewer=original.viewer;inspector.showMode(original.mode);
      if(original.mode==='body'||original.mode==='split')await inspector.inspectBody();
    })()`);
  }
  process.stdout.write('Native decrypted CONNECT body and opaque tunnel display verified.\n');
}
