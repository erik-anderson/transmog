import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";

/** Real WebView2 scrolling with requests captured through the native proxy. */
export async function smokeTrafficFollowing(call, evaluate, capture, screenshotPath) {
  const t = 'document.querySelector("app-shell").traffic';
  const saved = await evaluate(
    `(()=>{
      const shell=document.querySelector('app-shell'),w=shell.traffic;
      globalThis.__nativeTrafficFollowingFocus=w.getRootNode().activeElement;
      return {
        view:shell.activeView,sort:w.sort,label:w.sortLabel,filters:w.filters,groups:w.filterGroups,
        search:w.liveFilter,result:w.searchResultId,contentSearch:w.contentSearchActive,
        incident:w.incidentWindow,destinations:w.fetchDestinations,follow:w.followLatest,
        top:w.sessionScroller.scrollTop,left:w.sessionScroller.scrollLeft,
        selected:w.selectedSessionId,selection:[...w.trafficSelection.ids],
        focused:w.trafficSelection.focused,anchor:w.trafficSelection.anchor,
        summary:{method:w.selectedMethodText,url:w.selectedUrlText,status:w.selectedStatusText,tone:w.selectedTone}
      };
    })()`,
  );
  const wait = async (expression) => {
    const deadline = Date.now() + 15000;
    while (!(await evaluate(expression))) {
      if (Date.now() > deadline) {
        const state = await evaluate(
          `(()=>{const w=${t},s=w.sessionScroller;return {matched:w.totalMatched,follow:w.followLatest,pending:w.queryPending,loading:w.sessions.filter(row=>row.loading).length,top:s.scrollTop,height:s.clientHeight,total:s.scrollHeight,status:w.sessionText};})()`,
        );
        throw new Error(
          "Native traffic following timed out: " + expression + " · " + JSON.stringify(state),
        );
      }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  };
  const bottom = `${t}.followLatest && !${t}.queryPending && ${t}.sessions.every(row=>!row.loading) && ${t}.sessionScroller.scrollHeight-${t}.sessionScroller.clientHeight-${t}.sessionScroller.scrollTop<=1`;
  const click = async (selector) => {
    const point = await evaluate(
      `(()=>{const r=${t}.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`,
    );
    for (const type of ["mousePressed", "mouseReleased"])
      await call("Input.dispatchMouseEvent", { type, ...point, button: "left", clickCount: 1 });
  };
  const position = () =>
    evaluate(
      `(()=>{const w=${t};return {anchor:w.virtualList.anchor(w.sessionScroller.scrollTop),top:w.sessionScroller.scrollTop,selected:w.selectedSessionId,follow:w.followLatest};})()`,
    );
  const key = async (value, code, windowsVirtualKeyCode, modifiers = 0) => {
    for (const type of ["rawKeyDown", "keyUp"])
      await call("Input.dispatchKeyEvent", {
        type,
        key: value,
        code,
        windowsVirtualKeyCode,
        modifiers,
      });
  };
  try {
    await evaluate(
      `(async()=>{const shell=document.querySelector('app-shell');shell.shadowRoot.querySelector('a[data-view="traffic"]').click();await shell.$whenRendered();const w=shell.traffic;w.filters=[];w.filterGroups=[];w.fetchDestinations=[];w.searchResultId=null;w.contentSearchActive=false;w.incidentWindow=null;w.liveFilter='/native-following/';w.queryRevision++;await w.resumeLatest();})()`,
    );
    await Promise.all(
      Array.from({ length: 60 }, (_, index) => capture("/native-following/" + index)),
    );
    await wait(`${t}.totalMatched===60 && ${bottom}`);
    await evaluate(`${t}.trafficTable.focus({preventScroll:true})`);
    await key("a", "KeyA", 65, 2);
    await wait(`!${t}.followLatest && ${t}.selectedTrafficCount===60`);
    await key("Escape", "Escape", 27);
    await key("F5", "F5", 116);
    await wait(bottom);
    await key("Home", "Home", 36, 2);
    await wait(
      `!${t}.followLatest && ${t}.trafficSelection.focused===${t}.virtualList.ids[0] && ${t}.sessionScroller.scrollTop===0 && ${t}.getRootNode().activeElement?.id==='session-'+${t}.virtualList.ids[0]`,
    );
    assert.equal(
      await evaluate(`${t}.selectedTrafficCount`),
      0,
      "Native Ctrl+Home selected a request",
    );
    await click('button[aria-keyshortcuts="F5"]');
    await wait(bottom);
    const scroller = await evaluate(
      `(()=>{const s=${t}.sessionScroller,r=s.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2,scrollable:s.scrollHeight>s.clientHeight};})()`,
    );
    assert.equal(scroller.scrollable, true, "Native following fixture did not overflow");
    await call("Input.dispatchMouseEvent", {
      type: "mouseWheel",
      x: scroller.x,
      y: scroller.y,
      deltaX: 0,
      deltaY: -220,
    });
    await wait(`!${t}.followLatest && !${t}.rowRequests.size`);
    const paused = await position();
    await Promise.all(
      Array.from({ length: 20 }, (_, index) => capture("/native-following/" + (60 + index))),
    );
    await wait(`${t}.totalMatched===80 && !${t}.queryPending && !${t}.rowRequests.size`);
    assert.deepEqual(await position(), paused, "Native arrivals moved paused traffic");
    await click('button[aria-keyshortcuts="F5"]');
    await wait(bottom);
    const latestId = await evaluate(`${t}.sessions.at(-1).id`);
    await click("#session-" + latestId);
    await wait(`!${t}.followLatest`);
    const selected = await position();
    await capture("/native-following/80");
    await wait(`${t}.totalMatched===81 && !${t}.queryPending && !${t}.rowRequests.size`);
    assert.deepEqual(await position(), selected, "Native arrivals moved the selected request");
    await click('button[aria-keyshortcuts="F5"]');
    await wait(bottom);
    if (screenshotPath) {
      const shot = await call("Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshotPath.replace(/\.png$/i, "-traffic-following.png"),
        Buffer.from(shot.data, "base64"),
      );
    }
    process.stdout.write(
      "Native traffic appending, keyboard navigation, pause on scrolling/selection and Follow newest verified.\n",
    );
  } finally {
    await evaluate(
      `(async()=>{
        const shell=document.querySelector('app-shell'),w=shell.traffic,s=${JSON.stringify(saved)};
        try {
          const view=await window.__TAURI_INTERNALS__.invoke('query_traffic_view',{query:{search:'/native-following/'}});
          try {
            if(view.ids.length)await window.__TAURI_INTERNALS__.invoke('remove_traffic_entries',{ids:view.ids,restore:false});
          } finally {
            await window.__TAURI_INTERNALS__.invoke('release_traffic_view',{viewId:view.id});
          }
        } finally {
          w.scrollGeneration++;w.inspectionGeneration++;
          w.sort=s.sort;w.sortLabel=s.label;w.rebuildColumns();
          w.filters=s.filters;w.filterGroups=s.groups;w.liveFilter=s.search;
          w.searchResultId=s.result;w.contentSearchActive=s.contentSearch;
          w.incidentWindow=s.incident;w.fetchDestinations=s.destinations;
          w.followLatest=s.follow;w.selectedSessionId=s.selected;w.selectedDetail=null;
          w.trafficSelection.ids=new Set(s.selection);w.trafficSelection.focused=s.focused;
          w.trafficSelection.anchor=s.anchor;w.updateTrafficSelection();w.queryRevision++;
          try {
            await w.refreshSessions(undefined,true);
            if(s.selected)await w.refreshInspection();
            else {
              w.selectedMethodText=s.summary.method;w.selectedUrlText=s.summary.url;
              w.selectedStatusText=s.summary.status;w.selectedTone=s.summary.tone;
              w.$emit('selection-changed',null);
            }
            w.renderFollowState();await w.$whenRendered();
            if(!s.follow)w.sessionScroller.scrollTop=s.top;
            w.sessionScroller.scrollLeft=s.left;
            shell.shadowRoot.querySelector('a[data-view="'+s.view+'"]').click();
            const focus=globalThis.__nativeTrafficFollowingFocus;
            if(focus?.isConnected)focus.focus({preventScroll:true});
            else if(s.view==='traffic')w.trafficTable.focus({preventScroll:true});
            else shell.shadowRoot.querySelector('a[data-view="'+s.view+'"]').focus({preventScroll:true});
          } finally {
            delete globalThis.__nativeTrafficFollowingFocus;
          }
        }
      })()`,
    );
    const restored = await evaluate(
      `(()=>{const shell=document.querySelector('app-shell'),w=shell.traffic;return {view:shell.activeView,selected:w.selectedSessionId,selection:[...w.trafficSelection.ids],result:w.searchResultId,contentSearch:w.contentSearchActive,follow:w.followLatest};})()`,
    );
    assert.deepEqual(
      restored,
      {
        view: saved.view,
        selected: saved.selected,
        selection: saved.selection,
        result: saved.result,
        contentSearch: saved.contentSearch,
        follow: saved.follow,
      },
      "Native traffic probe did not restore the previous workspace",
    );
  }
}
