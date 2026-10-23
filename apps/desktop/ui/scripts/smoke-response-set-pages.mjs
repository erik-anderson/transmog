import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { nativeResponseSetContext } from "./native-response-set-helpers.mjs";

export async function smokeResponseSetPages(options) {
  const { main, directory, restart = false } = options,
    h = nativeResponseSetContext(options),
    { shell, a, sets, traffic, invoke } = h,
    picker = shell + ".responseSetPagePicker",
    dialog = picker + ".pageDialog";
  let status = await invoke("automation_status");
  if (!status.responseSets.some((set) => set.id === "native-pages"))
    status = await invoke("change_response_set", {
      generation: status.generation,
      change: {
        action: "create",
        id: "native-pages",
        name: "Native page association",
        notes: "Explicit captured identity",
        rules: [],
      },
    });
  await h.openSet(status, "native-pages");
  const change = () =>
      `[...${sets}.querySelectorAll('.set-page button')].find(node=>node.textContent.trim()==='Choose page…'||node.textContent.trim()==='Change…')`,
    use = h.button(dialog, "Use page");
  if (restart) {
    await h.wait(`${sets}.pageAvailability==='Capture unavailable'`);
    const remembered = status.responseSets.find((set) => set.id === "native-pages").previewPage;
    await h.pointer(change());
    await h.wait(dialog + ".open");
    assert.equal(await main.evaluate(picker + ".selectedId"), "");
    assert.equal(await main.evaluate(picker + ".rememberedUrl"), remembered.url);
    await h.key("Escape");
    assert.deepEqual(await invoke("automation_status"), status);
    await writeFile(
      join(directory, "native-pages-restart.json"),
      JSON.stringify({ remembered, unavailable: true }, null, 2),
    );
    return;
  }
  const before = structuredClone(status),
    text =
      "<html><head><title>Owned page association</title></head><body><h1>Captured page</h1></body></html>",
    fixture = join(directory, "pages.har"),
    entry = (url) => ({
      startedDateTime: "2026-10-18T18:00:00Z",
      time: 1,
      request: {
        method: "GET",
        url,
        httpVersion: "HTTP/1.1",
        headers: [],
        queryString: [],
        cookies: [],
        headersSize: -1,
        bodySize: 0,
      },
      response: {
        status: 200,
        statusText: "OK",
        httpVersion: "HTTP/1.1",
        headers: [{ name: "Content-Type", value: "text/html" }],
        cookies: [],
        content: { size: text.length, mimeType: "text/html", text },
        redirectURL: "",
        headersSize: -1,
        bodySize: text.length,
      },
      cache: {},
      timings: { send: 0, wait: 0, receive: 1 },
    });
  await writeFile(
    fixture,
    JSON.stringify({
      log: {
        version: "1.2",
        creator: { name: "Native page probe", version: "1" },
        entries: [
          "https://native-pages.invalid/",
          "https://native-pages.invalid/" + "long/".repeat(30),
          "https://native-pages.invalid/",
        ].map(entry),
      },
    }),
  );
  await main.evaluate(`${traffic}.importPaths([${JSON.stringify(fixture)}])`);
  assert.equal(
    (await invoke("query_sessions", { query: { search: "native-pages.invalid", limit: 10 } }))
      .sessions.length,
    3,
  );
  await h.pointer(change());
  await h.wait(dialog + ".open");
  assert.equal(await main.evaluate(picker + ".selectedId"), "");
  assert.equal(await main.evaluate(`${dialog}.querySelectorAll('input:checked').length`), 0);
  const choose = async () => {
    const index = await main.evaluate(
      `${picker}.pages.findIndex(page=>page.url==='https://native-pages.invalid/')`,
    );
    assert.ok(index >= 0);
    await h.pointer(dialog + `.querySelectorAll('input[type=radio]')[${index}]`);
  };
  await choose();
  // WebUI captures the handler at hydration. Wrap its dispatch, preserving the
  // real usePage handler, and intercept only its synchronous invoke registration.
  // Restore both methods before awaiting the native result so status refreshes
  // cannot replace the save's delayed reply.
  const delay = async (predicate) =>
      main.evaluate(`(()=>{
    const node=${picker},method=Object.getOwnPropertyDescriptor(node,'$callEventHandler'),dispatch=node.$callEventHandler,usePage=node.usePage;
    globalThis.releasePageReply=null;globalThis.pageDelayedCallbacks=0;
    node.$callEventHandler=function(handler,...args){
      if(handler!==usePage)return dispatch.call(this,handler,...args);
      const callbacks=window.__TAURI_INTERNALS__.callbacks,descriptor=Object.getOwnPropertyDescriptor(callbacks,'set'),original=callbacks.set;
      callbacks.set=function(id,callback){globalThis.pageDelayedCallbacks++;return original.call(this,id,result=>{
        if(${predicate})globalThis.releasePageReply=()=>callback(result);else callback(result);
      });};
      try{return dispatch.call(this,handler,...args);}
      finally{
        if(descriptor)Object.defineProperty(callbacks,'set',descriptor);else delete callbacks.set;
        if(method)Object.defineProperty(node,'$callEventHandler',method);else delete node.$callEventHandler;
      }
    };
  })()`),
    release = async () => {
      assert.equal(
        await main.evaluate("globalThis.pageDelayedCallbacks"),
        2,
        "Only the save success/error callback pair should be intercepted",
      );
      await main.evaluate("globalThis.releasePageReply();globalThis.releasePageReply=null");
    };
  await delay(
    "result?.responseSets?.some(set=>set.id==='native-pages'&&set.previewPageRevision===1)",
  );
  await h.pointer(use);
  await h.wait(`${picker}.saving&&globalThis.releasePageReply`);
  // A status refresh can return the same page revision while the save is held.
  // It must complete independently without replacing the save's delayed reply.
  await main.evaluate(
    "globalThis.pageRefreshComplete=false;window.__TAURI_INTERNALS__.invoke('automation_status').then(()=>{globalThis.pageRefreshComplete=true;});void 0",
  );
  await h.wait("globalThis.pageRefreshComplete");
  assert.equal(await main.evaluate(picker + ".saving"), true);
  await h.key("Tab");
  assert.equal(
    await main.evaluate(`${picker}.getRootNode().activeElement===${h.button(dialog, "Close")}`),
    true,
  );
  await release();
  await h.wait(`${picker}.pageSaved&&!${picker}.saving`);
  assert.equal(await main.evaluate(`${use}.disabled`), true);
  assert.equal(
    await main.evaluate(`${picker}.getRootNode().activeElement===${h.button(dialog, "Close")}`),
    true,
  );
  await h.key("Enter");
  await h.wait("!" + dialog + ".open");
  const associated = await invoke("automation_status"),
    owner = associated.responseSets.find((set) => set.id === "native-pages");
  assert.equal(associated.generation, before.generation);
  assert.deepEqual(owner.rules, []);
  assert.equal(owner.revision, 1);
  assert.equal(owner.applied, null);
  // A real compare-and-set conflict remains a failure after the modal closes.
  await h.pointer(change());
  await h.wait(dialog + ".open");
  await choose();
  await invoke("set_experiment_preview_page", {
    id: owner.id,
    revision: owner.previewPageRevision,
    page: owner.previewPage,
  });
  await delay("JSON.stringify(result).includes('The page association changed.')");
  await h.pointer(use);
  await h.wait(`${picker}.saving&&globalThis.releasePageReply`);
  await h.key("Escape");
  await release();
  await h.wait(
    `${shell}.noticeVisible&&${shell}.noticeTitleText==='Page association failed'&&!${picker}.saving`,
  );
  assert.equal(await main.evaluate(dialog + ".open"), false);
  assert.equal(await main.evaluate(`${sets}.getRootNode().activeElement===${change()}`), true);
  await h.pointer(`${shell}.shadowRoot.querySelector('button[aria-label="Dismiss notification"]')`);
  // An identical URL is a replacement only after a deliberate chooser choice.
  await invoke("remove_traffic_entries", { ids: [owner.previewPage.entryId], restore: false });
  await main.evaluate(sets + ".refreshPage()");
  await h.wait(`${sets}.pageAvailability==='Capture unavailable'`);
  await h.pointer(change());
  await h.wait(dialog + ".open");
  assert.equal(await main.evaluate(picker + ".selectedId"), "");
  assert.equal(await main.evaluate(picker + ".rememberedUrl"), owner.previewPage.url);
  await choose();
  const nextRevision =
    (await invoke("automation_status")).responseSets.find((set) => set.id === owner.id)
      .previewPageRevision + 1;
  await delay(
    `result?.responseSets?.some(set=>set.id==='native-pages'&&set.previewPageRevision===${nextRevision})`,
  );
  await h.pointer(use);
  await h.wait(`${picker}.saving&&globalThis.releasePageReply`);
  await h.key("Escape");
  await release();
  await h.wait(`!${picker}.saving`);
  assert.equal(await main.evaluate(traffic + ".previewPageDialog.open"), false);
  assert.equal(await main.evaluate(shell + ".activeView"), "automation");
  const final = await invoke("automation_status");
  assert.notEqual(
    final.responseSets.find((set) => set.id === owner.id).previewPage.entryId,
    owner.previewPage.entryId,
  );
  assert.equal(final.generation, before.generation);
  assert.deepEqual(final.rules, before.rules);
  const image = await main.call("Page.captureScreenshot", { format: "png" });
  await writeFile(join(directory, "native-pages.png"), Buffer.from(image.data, "base64"));
  await writeFile(
    join(directory, "native-pages-result.json"),
    JSON.stringify(
      {
        before,
        associated,
        final,
        explicitChoice: true,
        completedFocus: true,
        backgroundRefreshIndependent: true,
        staleCASFailure: true,
        canceledPendingNoLaunch: true,
      },
      null,
      2,
    ),
  );
  await main.evaluate(a + ".leaveResponseSet()");
  return { ownerId: owner.id, explicitChoice: true, staleCASFailure: true };
}
