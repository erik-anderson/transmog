import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";

/** Timings include the real retained-data commands and WebView2 renderer. */
export async function smokeTrafficPerformance(client, screenshot) {
  const report = await client.evaluate(`(async()=>{
    const t=document.querySelector('app-shell').traffic;
    const stats=values=>{values.sort((a,b)=>a-b);return {p50:values[Math.floor(values.length*.5)],p95:values[Math.floor(values.length*.95)]};};
    const render=[],refresh=[],queries=[],api=window.__TAURI_INTERNALS__;
    for(let sample=0;sample<30;sample++){
      const began=performance.now();for(let n=0;n<30;n++)t.renderViewport();
      if(sample>=5)render.push((performance.now()-began)/30);
      await new Promise(resolve=>setTimeout(resolve,0));
    }
    for(let sample=0;sample<12;sample++){
      const began=performance.now();await t.refreshSessions(undefined,true);
      if(sample>=2)refresh.push(performance.now()-began);
    }
    for(let sample=0;sample<12;sample++){
      const began=performance.now(),view=await api.invoke('query_traffic_view',{query:t.trafficQuery()});
      if(sample>=2)queries.push(performance.now()-began);
      await api.invoke('release_traffic_view',{viewId:view.id});
    }
    return {entries:t.virtualList.ids.length,domRows:t.sessions.length,renderMs:stats(render),refreshMs:stats(refresh),queryIpcMs:stats(queries),classificationColumns:t.extensionTableColumns?.length??0,labelCache:t.classifications?.cache.size??0};
  })()`);
  assert.equal(report.entries, 20000);
  assert.ok(report.domRows < 150);
  assert.equal(report.classificationColumns, 0);
  assert.equal(report.labelCache, 0);
  if (screenshot)
    await writeFile(
      screenshot.replace(/\.png$/i, "-traffic-performance.json"),
      JSON.stringify(report, null, 2) + "\n",
    );
  process.stdout.write("Native traffic performance: " + JSON.stringify(report) + "\n");
}
