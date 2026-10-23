import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

// Run only against an explicitly launched, uniquely owned test executable/profile.
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
const directory = resolve(process.argv[process.argv.indexOf("--output") + 1]);
assert.ok(Number.isInteger(port) && port > 1024);
await mkdir(directory, { recursive: true });
const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const target = targets.find((row) => row.type === "page" && /transmog|tauri/.test(row.url));
assert.ok(target, "Owned main WebView2 target missing");
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});
let sequence = 0;
const pending = new Map(),
  errors = [];
socket.addEventListener("message", ({ data }) => {
  const message = JSON.parse(String(data));
  if (message.id) {
    const request = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) request.reject(new Error(message.error.message));
    else request.resolve(message.result);
  } else if (message.method === "Runtime.exceptionThrown")
    errors.push(message.params.exceptionDetails);
});
const call = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++sequence;
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expression) => {
  const result = await call("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails)
    throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  return result.result.value;
};
const waitFor = async (expression) => {
  const deadline = Date.now() + 20000;
  while (!(await evaluate(`!!(${expression})`))) {
    if (Date.now() > deadline) throw new Error("Native wait timed out: " + expression);
    await new Promise((resolve) => setTimeout(resolve, 35));
  }
};
const shell = "document.querySelector('app-shell')",
  a = shell + ".automation",
  sets = a + ".responseSets",
  editor = a + ".autoResponseForm";
const button = (container, text) =>
  `[...${container}.querySelectorAll('button')].find(node=>node.textContent.trim()===${JSON.stringify(text)}&&node.getClientRects().length)`;
const pointer = async (expression) => {
  await waitFor(
    `(()=>{const node=${expression};return node&&node.getClientRects().length&&!node.disabled;})()`,
  );
  await evaluate(`${expression}.scrollIntoView({block:'center'})`);
  const point = await evaluate(
    `(()=>{const rect=${expression}.getBoundingClientRect();return {x:rect.x+rect.width/2,y:rect.y+rect.height/2};})()`,
  );
  for (const type of ["mouseMoved", "mousePressed", "mouseReleased"])
    await call("Input.dispatchMouseEvent", {
      type,
      button: "left",
      buttons: type === "mousePressed" ? 1 : 0,
      clickCount: 1,
      ...point,
    });
};
const key = async (key, code, vk, modifiers = 0) => {
  for (const type of ["keyDown", "keyUp"])
    await call("Input.dispatchKeyEvent", {
      type,
      key,
      code,
      windowsVirtualKeyCode: vk,
      modifiers,
      ...(key === "Enter" && type === "keyDown" ? { text: "\r" } : {}),
    });
};
const input = async (expression, text) => {
  await pointer(expression);
  assert.equal(
    await evaluate(
      `(()=>{const node=${expression};return node.getRootNode().activeElement===node;})()`,
    ),
    true,
    "Native input must receive pointer focus: " + expression,
  );
  await key("a", "KeyA", 65, 2);
  await call("Input.insertText", { text });
  assert.equal(
    await evaluate(`${expression}.value`),
    text,
    "Native input retains entered value: " + expression,
  );
};
const invoke = (command, args = {}) =>
  evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
const shot = async (name) => {
  const result = await call("Page.captureScreenshot", { format: "png" });
  await writeFile(resolve(directory, name), Buffer.from(result.data, "base64"));
};
try {
  await call("Page.enable");
  await call("Runtime.enable");
  await waitFor(`${shell}?.workspaceLoaded`);
  const before = await invoke("automation_status");
  assert.equal(before.responseSets.length, 0, "Native profile must be fresh");
  await pointer(`${shell}.shadowRoot.querySelector('a[data-view="automation"]')`);
  await waitFor(`${a}?.responseSets`);
  await pointer(button(a + ".querySelector('.workspace-tabs')", "Response sets"));
  await pointer(button(sets, "New response set…"));
  await waitFor(`${sets}.createDialog.open`);
  await input(sets + ".createName", "Native checkout validation");
  await input(sets + ".createNotes", "Notes from actual WebView2 input.");
  await pointer(button(sets + ".createDialog", "Create response set"));
  await waitFor(`${a}.collectionOpen`);
  const add = async () => {
    const count = await evaluate(`${sets}.draftRules.length`);
    await pointer(sets + ".addButton");
    await pointer(button(sets + ".addMenu", "Create from scratch…"));
    await waitFor(
      `${sets}.draftRules.length===${count + 1}&&!${a}.collectionAdding&&${a}.matchEditor.matchMode==='exact'`,
    );
  };
  await add();
  await input(editor + ".elements.namedItem('name')", "Checkout response");
  await input(a + ".matchEditor.addressInput", "https://native.invalid/checkout");
  await input(a + ".autoResponseBody", "First native body 😀");
  await add();
  await input(editor + ".elements.namedItem('name')", "Account response");
  await input(a + ".matchEditor.addressInput", "https://native.invalid/account");
  await input(a + ".autoResponseBody", "Second native body");
  await pointer(button(sets, "Checkout response"));
  assert.equal(await evaluate(a + ".autoResponseBody.value"), "First native body 😀");
  await key("s", "KeyS", 83, 2);
  await waitFor(`!${sets}.busy&&!${sets}.dirty&&!${a}.loadingSavedResponse`);
  const saved = await invoke("automation_status");
  assert.equal(saved.responseSets[0].rules.length, 2);
  assert.deepEqual(saved.rules, before.rules);
  assert.equal(saved.responseSets[0].applied, null);
  assert.equal(await evaluate(`${a}.collectionOpen`), true);
  const bodies = await Promise.all(
    saved.responseSets[0].rules.map((rule) =>
      invoke("inspect_response_asset", { reference: rule.request.responseAsset }),
    ),
  );
  assert.deepEqual(
    bodies.map((body) => Buffer.from(body.bytesBase64, "base64").toString()),
    ["First native body 😀", "Second native body"],
  );
  // Pending saves disable editor input and never claim focus moved onto a tab.
  await input(sets + ".notesInput", "Delayed native save");
  await evaluate(
    `(()=>{const callbacks=window.__TAURI_INTERNALS__.callbacks,original=callbacks.set;callbacks.set=function(id,callback){return original.call(this,id,result=>{if(result?.responseSets?.some(set=>set.notes==='Delayed native save'))globalThis.releaseWorkspaceSave=()=>callback(result);else callback(result);});};})()`,
  );
  await pointer(button(sets, "Save changes"));
  await waitFor(`globalThis.releaseWorkspaceSave&&${sets}.busy`);
  await pointer(button(a + ".querySelector('.workspace-tabs')", "Scripts"));
  assert.equal(
    await evaluate(
      `${a}.getRootNode().activeElement===${button(a + ".querySelector('.workspace-tabs')", "Scripts")}`,
    ),
    true,
  );
  await evaluate(
    "delete window.__TAURI_INTERNALS__.callbacks.set;globalThis.releaseWorkspaceSave()",
  );
  await waitFor(`!${sets}.busy`);
  assert.equal(
    await evaluate(
      `${a}.getRootNode().activeElement===${button(a + ".querySelector('.workspace-tabs')", "Scripts")}`,
    ),
    true,
  );
  assert.equal(await evaluate(`${a}.automationSection`), "experiments");
  await shot("native-saved-current.png");
  await pointer(`${sets}.querySelector('input[aria-label="Include Account response"]')`);
  assert.equal(await evaluate(`${sets}.dirty`), true);
  assert.equal((await invoke("automation_status")).responseSets[0].rules[1].enabled, true);
  await pointer(button(sets, "Discard changes"));
  await waitFor(`!${sets}.dirty`);
  await input(sets + ".notesInput", "Protected native draft");
  await pointer(sets + ".backButton");
  await waitFor(`${a}.unsavedDialog.open`);
  await key("Escape", "Escape", 27);
  assert.equal(await evaluate(sets + ".notesInput.value"), "Protected native draft");
  assert.equal(await evaluate(sets + ".dirty"), true);
  await shot("native-kept-draft.png");
  await pointer(sets + ".backButton");
  await waitFor(`${a}.unsavedDialog.open`);
  await pointer(button(a + ".unsavedDialog", "Discard changes"));
  await waitFor(`!${a}.collectionOpen`);
  assert.equal((await invoke("automation_status")).responseSets[0].notes, "Delayed native save");
  await pointer(button(sets, "Native checkout validation"));
  await waitFor(`${a}.collectionOpen`);
  await pointer(button(sets, "Account response"));
  await waitFor(`!${a}.loadingSavedResponse`);
  assert.equal(await evaluate(a + ".autoResponseBody.value"), "Second native body");
  const base = await invoke("automation_status"),
    set = base.responseSets[0];
  await input(sets + ".notesInput", "Unfinished native local notes");
  await invoke("change_response_set", {
    generation: base.generation,
    change: {
      action: "save-current",
      id: set.id,
      expectedRevision: set.revision,
      name: set.name,
      notes: "Externally saved native notes",
      rules: set.rules,
    },
  });
  await evaluate(a + ".refreshAutomation()");
  await pointer(button(sets, "Save changes"));
  await waitFor(`!${sets}.busy`);
  assert.match(await evaluate(sets + ".error"), /changed/);
  assert.equal(await evaluate(sets + ".notesInput.value"), "Unfinished native local notes");
  assert.equal(
    (await invoke("automation_status")).responseSets[0].notes,
    "Externally saved native notes",
  );
  await pointer(button(sets, "Discard changes"));
  await waitFor(`!${sets}.dirty`);
  // Notes-only Save from the departure dialog owns the complete final write.
  let latest = await invoke("automation_status");
  latest = await invoke("change_response_set", {
    generation: latest.generation,
    change: {
      action: "create",
      id: "native-pending-b",
      name: "Native second set",
      notes: "Saved second set",
      rules: [],
    },
  });
  await evaluate(`${a}.renderAutomation(${JSON.stringify(latest)})`);
  await input(sets + ".notesInput", "Native unsaved metadata save");
  await pointer(sets + ".backButton");
  await waitFor(`${a}.unsavedDialog.open`);
  await evaluate(
    `(()=>{const callbacks=window.__TAURI_INTERNALS__.callbacks,original=callbacks.set;callbacks.set=function(id,callback){return original.call(this,id,result=>{if(result?.responseSets?.some(set=>set.notes==='Native unsaved metadata save'))globalThis.releaseUnsavedSave=()=>callback(result);else callback(result);});};})()`,
  );
  await pointer(button(a + ".unsavedDialog", "Save changes"));
  await waitFor(`globalThis.releaseUnsavedSave&&${a}.resolvingUnsaved`);
  for (const label of ["Save changes", "Discard changes", "Keep editing"])
    assert.equal(await evaluate(`${button(a + ".unsavedDialog", label)}.disabled`), true);
  await key("Escape", "Escape", 27);
  await evaluate(
    `(async()=>{await ${a}.resolveUnsaved('discard');await ${a}.resolveUnsaved('stay');await ${a}.resolveUnsaved('save');await ${a}.openResponseSet(new CustomEvent('set-open',{detail:{id:'native-pending-b'}}));})()`,
  );
  assert.equal(await evaluate(`${a}.unsavedDialog.open&&${sets}.setId==='${set.id}'`), true);
  await shot("native-unsaved-save-pending.png");
  await evaluate("delete window.__TAURI_INTERNALS__.callbacks.set;globalThis.releaseUnsavedSave()");
  await waitFor(`!${a}.unsavedDialog.open&&!${a}.collectionOpen`);
  assert.equal(
    (await invoke("automation_status")).responseSets.find((item) => item.id === set.id).notes,
    "Native unsaved metadata save",
  );
  await pointer(button(sets, "Native second set"));
  await waitFor(`${sets}.setId==='native-pending-b'`);
  await input(sets + ".nameInput", "");
  await pointer(sets + ".backButton");
  await pointer(button(a + ".unsavedDialog", "Save changes"));
  await waitFor(`!${a}.resolvingUnsaved`);
  assert.match(
    await evaluate(
      `${a}.unsavedDialog.querySelector('p[role="alert"]:not([hidden])')?.textContent`,
    ),
    /Enter a response set name/,
  );
  await shot("native-unsaved-validation-error.png");
  await pointer(button(a + ".unsavedDialog", "Keep editing"));
  await input(sets + ".nameInput", "Native second set");
  await input(sets + ".notesInput", "Local second set draft");
  latest = await invoke("automation_status");
  const second = latest.responseSets.find((item) => item.id === "native-pending-b");
  latest = await invoke("change_response_set", {
    generation: latest.generation,
    change: {
      action: "save-current",
      id: second.id,
      expectedRevision: second.revision,
      name: second.name,
      notes: "External second set notes",
      rules: second.rules,
    },
  });
  await evaluate(`${a}.renderAutomation(${JSON.stringify(latest)})`);
  await pointer(sets + ".backButton");
  await pointer(button(a + ".unsavedDialog", "Save changes"));
  await waitFor(`!${a}.resolvingUnsaved`);
  assert.match(
    await evaluate(
      `${a}.unsavedDialog.querySelector('p[role="alert"]:not([hidden])')?.textContent`,
    ),
    /changed/,
  );
  await pointer(button(a + ".unsavedDialog", "Keep editing"));
  assert.equal(await evaluate(sets + ".notesInput.value"), "Local second set draft");
  await pointer(button(sets, "Discard changes"));
  // Seed a real linked response file; the Check response file and Responses
  // controls below run through their actual native command and pointer paths.
  const linkedPath = resolve(directory, "native-linked.txt");
  await writeFile(linkedPath, "Native linked bytes");
  const asset = await invoke("import_response_asset", {
    input: {
      id: "native-linked-asset",
      revision: 1,
      status: 200,
      headers: [],
      bodyPath: linkedPath,
      bodyFileMode: "linked",
      mediaType: "text/plain",
    },
  });
  latest = await invoke("automation_status");
  const destination = latest.responseSets.find((item) => item.id === "native-pending-b"),
    linkedRule = {
      ...saved.responseSets[0].rules[0],
      id: "native-linked-rule",
      displayName: "Linked native file",
      revision: 1,
      priority: 0,
      request: {
        ...saved.responseSets[0].rules[0].request,
        responseAsset: asset.id + "@" + asset.revision,
      },
    };
  latest = await invoke("change_response_set", {
    generation: latest.generation,
    change: {
      action: "save-current",
      id: destination.id,
      expectedRevision: destination.revision,
      name: destination.name,
      notes: destination.notes,
      rules: [linkedRule],
    },
  });
  await evaluate(`${a}.renderAutomation(${JSON.stringify(latest)})`);
  await waitFor(
    `${sets}.status.responseSets.find(set=>set.id==='native-pending-b').rules.length===1`,
  );
  await pointer(sets + ".backButton");
  await waitFor(`!${a}.collectionOpen`);
  await pointer(button(sets, "Native second set"));
  await evaluate(
    `(()=>{const callbacks=window.__TAURI_INTERNALS__.callbacks,original=callbacks.set;callbacks.set=function(id,callback){return original.call(this,id,result=>{if(result?.path===${JSON.stringify(linkedPath)}&&typeof result.bytes==='number')globalThis.releaseNativeFileCheck=()=>callback(result);else callback(result);});};})()`,
  );
  await pointer(button(sets, "Linked native file"));
  await waitFor(
    `globalThis.releaseNativeFileCheck&&${a}.fileChoiceBusy&&!${a}.loadingSavedResponse`,
  );
  assert.equal(await evaluate(`${button(editor, "Responses")}.disabled`), true);
  await evaluate(a + ".cancelAutoResponseEdit()");
  assert.equal(await evaluate(a + ".editorHidden"), false);
  await evaluate(
    "delete window.__TAURI_INTERNALS__.callbacks.set;globalThis.releaseNativeFileCheck()",
  );
  await waitFor(`!${a}.fileChoiceBusy`);
  await pointer(button(editor, "Responses"));
  assert.equal(await evaluate(a + ".editorHidden"), true);
  assert.equal(await evaluate(sets + ".addButton.disabled"), false);
  await shot("native-file-check-recovered.png");
  await shot("native-workspace.png");
  assert.deepEqual(errors, []);
  await writeFile(
    resolve(directory, "native-result.json"),
    JSON.stringify(
      {
        before,
        saved,
        final: await invoke("automation_status"),
        exactBodies: true,
        actualPointerAndKeyboard: true,
        delayedFocusPreserved: true,
        unsavedSaveLocked: true,
        modalValidationAndConflictErrors: true,
        fileCheckBusyRecovered: true,
        runtimeErrors: errors,
      },
      null,
      2,
    ),
  );
  process.stdout.write(
    "Native response set creation, whole drafts, exact bodies, CtrlS, delayed focus, Discard and departure verified.\n",
  );
} catch (error) {
  await shot("native-failure.png");
  await writeFile(
    resolve(directory, "native-failure.json"),
    JSON.stringify(
      {
        message: String(error),
        errors,
        state: await evaluate(
          `({section:${a}?.automationSection,busy:${sets}?.busy,error:${sets}?.error,responseError:${a}?.autoResponseError,invalid:[...${editor}.elements].filter(node=>node.validity&&!node.validity.valid).map(node=>({name:node.name,value:node.value,message:node.validationMessage})),matcher:${a}.matchEditor.captureDraft(),drafts:[...${a}.collectionSnapshots]})`,
        ),
      },
      null,
      2,
    ),
  );
  throw error;
} finally {
  socket.close();
}
