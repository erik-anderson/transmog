import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { gzipSync } from "node:zlib";
import { nativeFixture } from "./native-fixture.mjs";

// Native editor events and menus are exercised with a scoped write sink. The
// system clipboard is left untouched: this host could not safely snapshot its
// native DataObject. A bubble guard blocks the browser copy default if the
// source's empty-selection key handler regresses, while retaining that failure.
async function interceptClipboard(evaluate) {
  await evaluate(`(()=>{
    const probe={descriptor:Object.getOwnPropertyDescriptor(navigator,'clipboard'),text:'',copyKey:null,blockedByProbe:false};
    probe.capture=event=>{
      if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='c'){
        probe.copyKey=event;probe.blockedByProbe=false;
      }
    };
    probe.guard=event=>{
      if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='c'&&!event.defaultPrevented){
        probe.blockedByProbe=true;event.preventDefault();event.stopImmediatePropagation();
      }
    };
    document.addEventListener('keydown',probe.capture,true);document.addEventListener('keydown',probe.guard);
    Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>{probe.text=text;}}});
    globalThis.__bodySourceClipboard=probe;
  })()`);
  return {
    readText: () => evaluate("globalThis.__bodySourceClipboard.text"),
    seed: (text) => evaluate("globalThis.__bodySourceClipboard.text=" + JSON.stringify(text)),
    restore: () =>
      evaluate(`(()=>{const probe=globalThis.__bodySourceClipboard;if(!probe)return;
      document.removeEventListener('keydown',probe.capture,true);document.removeEventListener('keydown',probe.guard);
      if(probe.descriptor)Object.defineProperty(navigator,'clipboard',probe.descriptor);else delete navigator.clipboard;
      delete globalThis.__bodySourceClipboard;
    })()`),
  };
}

/** Real retained evidence, WebView2 input, source copy events and host menus. */
export async function smokeBodySource(viewer, traffic, directory, screenshotPath) {
  const { call, evaluate } = viewer,
    inspector = `${traffic}.querySelector('message-inspector[side="response"]')`,
    source = `${inspector}.querySelector('body-source-viewer')`,
    menu = "document.querySelector('inspection-copy-menu').shadowRoot.querySelector('[role=menu]')";
  const wait = async (expression, message = expression) =>
    evaluate(
      `(async()=>{const deadline=Date.now()+15000;while(!(${expression})&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,25));if(!(${expression}))throw new Error(${JSON.stringify("Native body source timed out: " + message)});})()`,
    );
  const key = async (name, code, modifiers = 0) => {
    for (const type of ["keyDown", "keyUp"])
      await call("Input.dispatchKeyEvent", {
        type,
        key: name,
        windowsVirtualKeyCode: code,
        modifiers,
      });
  };
  const pointer = async (point, button = "left") => {
    await call("Input.dispatchMouseEvent", { type: "mouseMoved", ...point });
    for (const type of ["mousePressed", "mouseReleased"])
      await call("Input.dispatchMouseEvent", { type, ...point, button, clickCount: 1 });
  };
  const point = async (expression) =>
    evaluate(
      `(()=>{const r=(${expression}).getBoundingClientRect();return {x:r.left+r.width/2,y:r.top+r.height/2};})()`,
    );
  const menuLabels = () =>
    evaluate(`[...${menu}.querySelectorAll('button')].map(button=>button.textContent.trim())`);
  const openKeyboard = async () => {
    await evaluate(`${source}.editor.focus()`);
    await key("F10", 121, 8);
    await wait(`${menu}.matches(':popover-open')`, "keyboard inspection menu");
    assert.equal(
      await evaluate(
        `(()=>{const r=${menu}.getBoundingClientRect();return r.width>0&&r.left>=0&&r.top>=0&&r.right<=innerWidth&&r.bottom<=innerHeight;})()`,
      ),
      true,
      "Keyboard source menu leaves the native viewport",
    );
  };
  const copy = async (label) => {
    const labels = await menuLabels();
    assert.ok(
      labels.includes(label),
      `Missing native source action ${label}: ${JSON.stringify(labels)}`,
    );
    await pointer(
      await point(
        `[...${menu}.querySelectorAll('button')].find(button=>button.textContent.trim()===${JSON.stringify(label)})`,
      ),
    );
    await wait(`!${menu}.matches(':popover-open')`, label + " completion");
    return clipboard.readText();
  };
  const capture = async (label) => {
    if (screenshotPath)
      await writeFile(
        screenshotPath.replace(/\.png$/i, "") + `-body-source-${label}.png`,
        Buffer.from(
          (await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false }))
            .data,
          "base64",
        ),
      );
  };
  const raw =
      ")]}'\n" +
      '{"duplicate":1,"duplicate":2,"exact":9.007199254740993e15,"scale":1.2300e+04,"escaped":"line\\nCafé 中",' +
      '"markup":"<script>globalThis.nativeBodySourceExecuted=true</script>","a/b":{"~leaf":"semantic Café"},"empty":"","null":null,"items":[' +
      Array.from({ length: 180 }, (_, index) =>
        JSON.stringify({ id: index, name: "Retained row " + index, nested: { active: true } }),
      ).join(",") +
      '],"tail":"native-offscreen-source-sentinel"}\r\n',
    compressed = gzipSync(Buffer.from(raw)),
    fixturePath = join(directory, "body-source-fixture.tmcap"),
    fixtureUrl = "https://native-source.invalid/body-source-fixture";
  await mkdir(directory, { recursive: true });
  await writeFile(
    fixturePath,
    nativeFixture(
      [
        {
          kind: "request-head",
          payload: { boundary: "client-request", method: "GET", target: fixtureUrl, headers: [] },
        },
        {
          kind: "response-head",
          payload: {
            boundary: "client-response",
            status: 200,
            headers: [
              ["Content-Type", "application/json; charset=utf-8"],
              ["Content-Encoding", "gzip"],
            ].map(([name, value]) => ({
              name: Array.from(Buffer.from(name)),
              value: Array.from(Buffer.from(value)),
            })),
          },
        },
        {
          kind: "body-segment",
          payload: {
            boundary: "client-response",
            byte_count: compressed.length,
            bytes: Array.from(compressed),
            truncated: false,
          },
        },
        { kind: "completed", payload: null },
      ],
      731,
    ),
  );
  const clipboard = await interceptClipboard(evaluate);
  let saved = false,
    failure = null;
  try {
    await evaluate(`(()=>{
      const w=${traffic},i=${inspector};
      globalThis.__bodySourceSmoke={selectedId:w.selectedSessionId,side:w.inspectorSide,expanded:w.inspectorExpanded,
        mode:i.mode,viewer:i.viewer,decode:i.decodeContent,sourceMode:i.bodySourceMode,sourceWrap:i.bodySourceWrap,
        limit:i.bodyLimit.value,optionsOpen:i.bodyOptions.open,importStatus:w.importStatus,
        focus:document.querySelector('app-shell').shadowRoot.activeElement,ids:[],count:w.totalMatched};
    })()`);
    saved = true;
    await evaluate(`${traffic}.importPaths([${JSON.stringify(fixturePath)}])`);
    await evaluate(`(async()=>{
      const w=${traffic},page=await window.__TAURI_INTERNALS__.invoke('query_sessions',{query:{search:'native-source.invalid',limit:10}}),row=page.sessions.find(row=>row.url===${JSON.stringify(fixtureUrl)});if(!row)throw new Error('Retained source fixture was not imported');globalThis.__bodySourceSmoke.ids=[row.id];if(!await w.revealSession(row.id))throw new Error('Retained source fixture could not be revealed');
    })()`);
    await wait(
      `${traffic}.selectedDetail?.id===globalThis.__bodySourceSmoke.ids[0]`,
      "fixture selection",
    );
    await evaluate(`(async()=>{
      const w=${traffic},i=${inspector};w.showInspectorSide('response');if(!w.inspectorExpanded)w.toggleInspector();
      i.viewer='auto';i.decodeContent=true;i.bodySourceMode='source';i.showMode('body');await i.inspectBody();w.$flushUpdates();i.$flushUpdates();
    })()`);
    await wait(
      `${source}?.sourceEditorReady&&${source}.editor&&${source}.model`,
      "Monaco readiness",
    );
    const initial =
      await evaluate(`(()=>{const v=${source};return {preview:v.preview,display:v.bodySourceText,language:v.model.getLanguageId(),
      readonly:v.editor.getRawOptions().readOnly,fallbackHidden:v.fallback.hidden,lines:v.model.getLineCount(),
      decoded:${inspector}.bodySummary.includes('content decoded'),notice:${inspector}.bodyNotice};})()`);
    assert.equal(initial.preview.formatted, true);
    assert.equal(initial.preview.truncated, false);
    assert.equal(
      initial.preview.originalText,
      raw,
      "Decoded original text was reconstructed from formatted JSON",
    );
    assert.equal(initial.language, "json");
    assert.equal(initial.readonly, true);
    assert.equal(initial.fallbackHidden, true);
    assert.equal(initial.decoded, true);
    assert.equal(
      initial.display.match(/"duplicate"/g)?.length,
      2,
      "Pretty source lost a duplicate key",
    );
    assert.ok(
      initial.display.includes("9.007199254740993e15"),
      "Pretty source rounded a number token",
    );
    assert.ok(initial.display.includes("1.2300e+04"), "Pretty source normalized a number token");
    assert.match(initial.notice, /XSSI prefix omitted/);
    assert.ok(initial.lines > 900, "Fixture does not exercise offscreen source lines");
    assert.equal(await evaluate("globalThis.nativeBodySourceExecuted===undefined"), true);
    await wait(
      `new Set([...${source}.editorHost.querySelectorAll('.view-line span span')].map(node=>getComputedStyle(node).color)).size>1`,
      "visible syntax highlighting",
    );
    assert.deepEqual(
      await evaluate(
        `[...${inspector}.querySelectorAll('[role=group][aria-label="JSON body view"] button')].map(button=>({label:button.textContent.trim(),pressed:button.getAttribute('aria-pressed')}))`,
      ),
      [
        { label: "Source", pressed: "true" },
        { label: "Fields", pressed: "false" },
      ],
      "Source/Fields controls do not expose the active source view",
    );
    await evaluate(`${source}.editor.focus()`);
    await call("Input.insertText", { text: "native-readonly-must-not-be-inserted" });
    assert.equal(await evaluate(`${source}.model.getValue()`), initial.display);

    const selected = await evaluate(`(()=>{
      const v=${source},line=v.model.getLinesContent().findIndex(line=>line.includes('"duplicate"'))+1;
      v.editor.setSelection({startLineNumber:line,startColumn:3,endLineNumber:line,endColumn:14});v.editor.revealLine(line);v.editor.focus();
      const p=v.editor.getScrolledVisiblePosition({lineNumber:line,column:6}),r=v.editorHost.getBoundingClientRect();
      return {text:v.model.getValueInRange(v.editor.getSelection()),point:{x:r.left+p.left+1,y:r.top+p.top+p.height/2}};
    })()`);
    await pointer(selected.point, "right");
    await wait(`${menu}.matches(':popover-open')`, "pointer inspection menu");
    const selectedActions = await menuLabels();
    assert.equal(selectedActions[0], "Copy");
    for (const label of [
      "Copy body preview",
      "Copy original text preview",
      "Open in Format converter",
    ])
      assert.ok(selectedActions.includes(label), "Missing source action: " + label);
    assert.equal(await copy("Copy"), selected.text);
    assert.equal(
      await evaluate(`${source}.model.getValueInRange(${source}.editor.getSelection())`),
      selected.text,
    );
    await wait(`${source}.editor.hasTextFocus()`, "selection-copy return focus");
    const copiedEvent = await evaluate(`(()=>{
      const v=${source},data=new DataTransfer(),event=new ClipboardEvent('copy',{clipboardData:data,bubbles:true,cancelable:true,composed:true});
      v.editorHost.querySelector('.native-edit-context,textarea.inputarea').dispatchEvent(event);
      return {prevented:event.defaultPrevented,text:data.getData('text/plain')};
    })()`);
    assert.deepEqual(copiedEvent, { prevented: true, text: selected.text });

    await openKeyboard();
    assert.equal(await copy("Copy original text preview"), raw);
    await evaluate(
      `(async()=>{const v=${source};await v.editor.getAction('editor.foldAll').run();v.editor.setPosition({lineNumber:1,column:1});v.editor.focus();})()`,
    );
    const folded = await evaluate(
      `(()=>{const v=${source};return {rendered:v.editorHost.querySelectorAll('.view-line').length,lines:v.model.getLineCount(),hasTail:v.editorHost.querySelector('.view-lines').textContent.includes('native-offscreen-source-sentinel')};})()`,
    );
    assert.ok(folded.rendered < folded.lines / 2, "Source was not folded or virtualized");
    assert.equal(folded.hasTail, false);
    await openKeyboard();
    assert.equal(
      await copy("Copy body preview"),
      initial.display,
      "Folded/offscreen text was omitted",
    );
    const sentinel = "native-empty-source-copy-sentinel";
    await clipboard.seed(sentinel);
    await evaluate(
      `${source}.editor.setPosition({lineNumber:1,column:1});${source}.editor.focus()`,
    );
    await key("c", 67, 2);
    assert.equal(
      await evaluate(
        "globalThis.__bodySourceClipboard.copyKey?.defaultPrevented&&!globalThis.__bodySourceClipboard.blockedByProbe",
      ),
      true,
      "The source did not prevent native Ctrl+C with an empty selection",
    );
    assert.equal(
      await clipboard.readText(),
      sentinel,
      "Empty source Ctrl+C changed the intercepted clipboard text",
    );
    await openKeyboard();
    const emptyActions = await menuLabels();
    assert.equal(emptyActions.includes("Copy"), false);
    assert.equal(emptyActions.includes("Open in Format converter"), false);
    assert.ok(emptyActions.includes("Copy body preview"));
    assert.ok(emptyActions.includes("Copy original text preview"));
    await key("End", 35);
    assert.equal(
      await evaluate(
        "document.querySelector('inspection-copy-menu').shadowRoot.activeElement.textContent.trim()",
      ),
      emptyActions.at(-1),
    );
    await key("Home", 36);
    assert.equal(
      await evaluate(
        "document.querySelector('inspection-copy-menu').shadowRoot.activeElement.textContent.trim()",
      ),
      emptyActions[0],
    );
    await key("Escape", 27);
    await wait(`${source}.editor.hasTextFocus()`, "keyboard-menu Escape return focus");

    await evaluate(
      `(async()=>{const v=${source};await v.editor.getAction('editor.unfoldAll').run();v.editor.setPosition({lineNumber:1,column:1});v.editor.setScrollTop(0);})()`,
    );
    const sourceContextAt = async (name, occurrence = 0) => {
      let target;
      try {
        target = await evaluate(`(async()=>{
        const v=${source},text=v.model.getValue(),token=JSON.stringify(${JSON.stringify(name)});
        let offset=-1;for(let index=0;index<=${occurrence};index++)offset=text.indexOf(token,offset+1);
        if(offset<0)throw new Error('Semantic source target is unavailable');
        const position=v.model.getPositionAt(offset+1);v.editor.setPosition(position);v.editor.revealPositionInCenter(position,1);v.editor.focus();
        // Monaco can update folding/scroll state before its text DOM is painted.
        // These pointer actions must hit the key rather than rely on its caret.
        await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
        const deadline=Date.now()+3000;let diagnostic;
        do{const p=v.editor.getScrolledVisiblePosition(position),next=v.editor.getScrolledVisiblePosition(v.model.getPositionAt(offset+2)),glyphWidth=next&&next.top===p.top?Math.max(4,next.left-p.left):8,r=v.editorHost.getBoundingClientRect(),point={x:r.left+p.left+glyphWidth/2,y:r.top+p.top+p.height/2},hit=v.editor.getTargetAtClientPoint(point.x,point.y),hitOffset=hit?.position?v.model.getOffsetAt(hit.position):null;
          const domHit=v.getRootNode().elementFromPoint(point.x,point.y);
          diagnostic={position,scrolledPosition:p,point,hitType:hit?.type,hitPosition:hit?.position,hitOffset,expectedStart:offset,expectedEnd:offset+token.length,
            host:r.toJSON(),hostOffset:{width:v.editorHost.offsetWidth,height:v.editorHost.offsetHeight},editor:v.editor.getDomNode().getBoundingClientRect().toJSON(),layout:v.editor.getLayoutInfo(),visibleRanges:v.editor.getVisibleRanges(),viewport:{width:innerWidth,height:innerHeight},
            rendered:v.editorHost.querySelector('.view-lines')?.textContent.slice(0,300),domHit:domHit?{tag:domHit.tagName,class:domHit.className}:null,
            zoom:getComputedStyle(document.documentElement).zoom,scrollLeft:v.editor.getScrollLeft(),scrollTop:v.editor.getScrollTop(),copyFeedbackOpen:document.querySelector('inspection-copy-menu').feedback.matches(':popover-open')};
          if(hitOffset!==null&&hitOffset>=offset&&hitOffset<offset+token.length)return point;
          await new Promise(resolve=>requestAnimationFrame(resolve));
        }while(Date.now()<deadline);
        throw new Error('Native semantic pointer did not hit its source key: '+JSON.stringify(diagnostic));
      })()`);
      } catch (error) {
        await capture("semantic-pointer-failure");
        throw error;
      }
      await pointer(target, "right");
      await wait(`${menu}.matches(':popover-open')`, "semantic source menu");
    };
    await sourceContextAt("exact");
    assert.equal(await copy("Copy value"), "9.007199254740993e15");
    await sourceContextAt("scale");
    assert.equal(await copy("Copy as JSON"), "1.2300e+04");
    await sourceContextAt("escaped");
    assert.equal(await copy("Copy value"), "line\\nCafé 中");
    await sourceContextAt("~leaf");
    assert.equal(await copy("Copy JSON Pointer"), "/a~1b/~0leaf");
    await sourceContextAt("duplicate", 1);
    assert.equal(
      (await menuLabels()).includes("Copy JSON Pointer"),
      false,
      "Duplicate member exposed an ambiguous JSON Pointer",
    );
    assert.equal(await copy("Copy value"), "2");
    await evaluate(
      `${source}.editor.setPosition({lineNumber:1,column:1});${source}.editor.setScrollTop(0)`,
    );
    const layouts = [];
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await call("Emulation.setDeviceMetricsOverride", {
        width,
        height,
        deviceScaleFactor: 1,
        mobile: false,
      });
      await evaluate(`${traffic}.showInspectorSide('response');${traffic}.$flushUpdates()`);
      await wait(`${source}?.sourceEditorReady&&${source}.editor`, "source at " + width);
      await evaluate(`${source}.editor.layout()`);
      const layout = await evaluate(
        `(()=>{const v=${source},i=${inspector},r=v.editorHost.getBoundingClientRect(),controls=[i.bodyViewerControl,i.querySelector('.body-options > summary')].filter(Boolean);return {width:innerWidth,height:innerHeight,editor:r.toJSON(),controls:controls.map(node=>node.getBoundingClientRect().toJSON()),documentWidth:document.documentElement.scrollWidth};})()`,
      );
      assert.equal(layout.controls.length, 2, "Source body options are unavailable");
      assert.ok(layout.editor.width >= 280 && layout.editor.height >= 160, JSON.stringify(layout));
      assert.ok(
        layout.editor.left >= 0 &&
          layout.editor.right <= width + 1 &&
          layout.editor.bottom <= height + 1,
        JSON.stringify(layout),
      );
      assert.ok(
        layout.controls.every(
          (r) =>
            r.width > 0 &&
            r.left >= 0 &&
            r.right <= width + 1 &&
            r.top >= 0 &&
            r.bottom <= height + 1,
        ),
        JSON.stringify(layout),
      );
      assert.ok(
        layout.documentWidth <= width + 1,
        "Native source caused window overflow: " + JSON.stringify(layout),
      );
      layouts.push(layout);
      await capture(String(width));
      if (width === 800) {
        // Exercise the ordinary compact inspector as well as its expanded
        // surface: Monaco must scroll the caret inside the real clipped pane.
        await evaluate(`${traffic}.toggleInspector();${traffic}.$flushUpdates()`);
        await wait(`${source}?.sourceEditorReady&&${source}.editor`, "ordinary compact source");
        await evaluate(`${source}.editor.layout();${source}.editor.focus()`);
        await key("Home", 36, 2);
        for (let index = 0; index < 12; index++) await key("ArrowDown", 40);
        await evaluate(
          "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
        );
        const caret = await evaluate(
          `(()=>{const v=${source},position=v.editor.getPosition(),p=v.editor.getScrolledVisiblePosition(position),host=v.editorHost.getBoundingClientRect(),content=v.querySelector('.source-content').getBoundingClientRect(),body=${inspector}.querySelector('.body-scroll').getBoundingClientRect();return {line:position?.lineNumber,focused:v.editor.hasTextFocus(),caret:p?{left:host.left+p.left,top:host.top+p.top,bottom:host.top+p.top+p.height}:null,viewport:{left:Math.max(0,host.left,content.left,body.left),right:Math.min(innerWidth,host.right,content.right,body.right),top:Math.max(0,host.top,content.top,body.top),bottom:Math.min(innerHeight,host.bottom,content.bottom,body.bottom)}};})()`,
        );
        assert.ok(caret.focused && caret.line > 5 && caret.caret, JSON.stringify(caret));
        assert.ok(
          caret.caret.left >= caret.viewport.left - 1 &&
            caret.caret.left <= caret.viewport.right + 1 &&
            caret.caret.top >= caret.viewport.top - 1 &&
            caret.caret.bottom <= caret.viewport.bottom + 1,
          "Native compact source caret is clipped: " + JSON.stringify(caret),
        );
        await capture("compact-caret-800");
        await evaluate(`${traffic}.toggleInspector();${traffic}.$flushUpdates()`);
        await wait(`${source}?.sourceEditorReady&&${source}.editor`, "expanded source restoration");
      }
    }
    await call("Emulation.setDeviceMetricsOverride", {
      width: 1440,
      height: 900,
      deviceScaleFactor: 1,
      mobile: false,
    });
    const fields = `${source}.querySelector('extension-visualizer.json-fields')`,
      viewButton = (label) =>
        `[...${inspector}.querySelectorAll('[role=group][aria-label="JSON body view"] button')].find(button=>button.textContent.trim()===${JSON.stringify(label)})`;
    await pointer(await point(viewButton("Fields")));
    await wait(`${fields}?.visualizerRows.length>0&&!${fields}.visualizerBusy`, "JSON Fields tree");
    assert.equal(await evaluate(`(${viewButton("Fields")}).getAttribute('aria-pressed')`), "true");
    assert.equal(await evaluate(`(${viewButton("Source")}).getAttribute('aria-pressed')`), "false");
    assert.equal(
      await evaluate(`${source}.editor===null&&${source}.model===null`),
      true,
      "Fields retained the hidden source editor",
    );
    await wait(
      `${fields}.querySelector('button[aria-label="Expand JSON"]')`,
      "collapsed JSON root",
    );
    await pointer(await point(`${fields}.querySelector('button[aria-label="Expand JSON"]')`));
    await wait(
      `${fields}.visualizerRows.filter(row=>row.name==='duplicate').length===2`,
      "ordered duplicate Fields rows",
    );
    const fieldId = async (name, occurrence = 0) =>
      evaluate(
        `${source}.jsonDocument.root.children.filter(node=>node.name===${JSON.stringify(name)})[${occurrence}].id`,
      );
    const selectField = async (id, details = false) => {
      await evaluate(
        `${fields}.selectNode(${JSON.stringify(id)},${details});${fields}.$flushUpdates();${fields}.treeViewport.focus()`,
      );
    };
    const fieldKeyboard = async (id) => {
      await selectField(id);
      await key("F10", 121, 8);
      await wait(`${menu}.matches(':popover-open')`, "JSON Fields keyboard menu");
    };
    const exactId = await fieldId("exact"),
      duplicateId = await fieldId("duplicate", 1),
      escapedId = await fieldId("escaped"),
      emptyId = await fieldId("empty"),
      nullId = await fieldId("null"),
      itemsId = await fieldId("items"),
      rootId = await evaluate(`${source}.jsonDocument.root.id`),
      leafId = await evaluate(
        `[...${source}.jsonDocument.nodes.values()].find(node=>node.name==='~leaf').id`,
      );
    await fieldKeyboard(exactId);
    assert.deepEqual(await menuLabels(), [
      "Copy name",
      "Copy value",
      "Copy name and value",
      "Copy as JSON",
      "Copy JSON Pointer",
    ]);
    assert.equal(await copy("Copy name and value"), "exact: 9.007199254740993e15");
    await fieldKeyboard(exactId);
    assert.equal(await copy("Copy as JSON"), "9.007199254740993e15");
    await fieldKeyboard(escapedId);
    assert.equal(await copy("Copy value"), "line\\nCafé 中");
    await fieldKeyboard(escapedId);
    assert.equal(await copy("Copy as JSON"), '"line\\nCafé 中"');
    await fieldKeyboard(emptyId);
    assert.equal(await copy("Copy value"), "");
    await fieldKeyboard(nullId);
    assert.equal(await copy("Copy value"), "null");
    await fieldKeyboard(leafId);
    assert.equal(await copy("Copy JSON Pointer"), "/a~1b/~0leaf");

    // Pointer copying targets an occurrence without moving the tree selection.
    await selectField(exactId);
    const duplicateRow = `${fields}.visualizerRows.find(row=>row.id===${JSON.stringify(duplicateId)})`;
    await pointer(
      await point(
        `${fields}.querySelector('[id="'+(${duplicateRow}).domId+'"] .visualizer-field')`,
      ),
      "right",
    );
    await wait(`${menu}.matches(':popover-open')`, "duplicate Fields pointer menu");
    assert.equal((await menuLabels()).includes("Copy JSON Pointer"), false);
    assert.equal(await copy("Copy value"), "2");
    assert.equal(await evaluate(`${fields}.selected`), exactId);

    // A collapsed array copy includes all retained descendants, independently
    // of the tree's expansion set and virtual window.
    await selectField(itemsId);
    const beforeSubtree = await evaluate(
      `(()=>{const v=${fields};return {expanded:[...v.expanded],top:v.treeViewport.scrollTop,selected:v.selected};})()`,
    );
    await key("F10", 121, 8);
    await wait(`${menu}.matches(':popover-open')`, "collapsed JSON array copy");
    const subtree = await copy("Copy subtree");
    assert.match(subtree, /name: Retained row 179/);
    assert.match(subtree, /active: true/);
    assert.deepEqual(
      await evaluate(
        `(()=>{const v=${fields};return {expanded:[...v.expanded],top:v.treeViewport.scrollTop,selected:v.selected};})()`,
      ),
      beforeSubtree,
    );
    await fieldKeyboard(rootId);
    const rootActions = await menuLabels();
    for (const label of ["Copy name", "Copy value", "Copy name and value"])
      assert.equal(
        rootActions.includes(label),
        false,
        "JSON root offered an invented field action",
      );
    assert.equal(await copy("Copy as JSON"), raw.slice(5).trim());
    await fieldKeyboard(exactId);
    await key("Escape", 27);
    await wait(
      `${fields}.treeViewport===${fields}.getRootNode().activeElement`,
      "Fields menu Escape return focus",
    );
    await capture("fields-1440");
    await call("Emulation.setDeviceMetricsOverride", {
      width: 800,
      height: 600,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await fieldKeyboard(exactId);
    assert.equal(
      await evaluate(
        `(()=>{const r=${menu}.getBoundingClientRect();return r.width>0&&r.left>=0&&r.top>=0&&r.right<=innerWidth&&r.bottom<=innerHeight;})()`,
      ),
      true,
      "Compact Fields menu leaves the native viewport",
    );
    assert.equal(await copy("Copy JSON Pointer"), "/exact");
    await selectField(exactId, true);
    const showSource = `[...${fields}.querySelectorAll('.visualizer-node-heading button')].find(button=>button.textContent.trim()==='Show in source')`;
    assert.equal(
      await evaluate(
        `(()=>{const r=(${showSource}).getBoundingClientRect();return r.width>0&&r.height>0&&r.left>=0&&r.top>=0&&r.right<=innerWidth&&r.bottom<=innerHeight;})()`,
      ),
      true,
      "Show in source is unreachable in compact Fields",
    );
    await capture("fields-800");
    await pointer(await point(showSource));
    await wait(
      `${inspector}.bodySourceMode==='source'&&${source}.sourceEditorReady&&${source}.editor?.hasTextFocus()`,
      "Fields Show in source",
    );
    assert.equal(await evaluate(`(${viewButton("Source")}).getAttribute('aria-pressed')`), "true");
    assert.equal(await evaluate(`(${viewButton("Fields")}).getAttribute('aria-pressed')`), "false");
    assert.equal(
      await evaluate(`${source}.model.getValueInRange(${source}.editor.getSelection())`),
      "9.007199254740993e15",
    );
    assert.equal(
      await evaluate(`${source}.sourceSnapshot===null&&!${source}.querySelector('.json-fields')`),
      true,
      "Source retained the hidden Fields snapshot",
    );
    const modeButton = (label) =>
      `[...${inspector}.querySelectorAll('.message-tabs button')].find(button=>button.textContent.trim()===${JSON.stringify(label)})`;
    await pointer(await point(modeButton("Headers")));
    await wait(
      `${source}.editor===null&&${source}.model===null&&${source}.jsonDocument===null&&${source}.sourceSnapshot===null`,
      "hidden source/Fields disposal",
    );
    await pointer(await point(modeButton("Body")));
    await wait(
      `${source}.sourceEditorReady&&${source}.editor&&${source}.jsonDocument`,
      "source/Fields restoration",
    );
    assert.equal(
      await evaluate(`${source}.model.getValueInRange(${source}.editor.getSelection())`),
      "9.007199254740993e15",
    );
    // Switching to decoded Text must retain exact original syntax and offer a
    // single whole-preview action, rather than a second identical source action.
    await evaluate(
      `(async()=>{const i=${inspector};i.viewer='original-text';await i.inspectBody();i.$flushUpdates();})()`,
    );
    await wait(`${source}?.sourceEditorReady&&${source}.model`, "decoded Text source");
    assert.equal(await evaluate(`${source}.preview.text`), raw);
    await openKeyboard();
    const textActions = await menuLabels();
    assert.ok(textActions.includes("Copy body preview"));
    assert.equal(textActions.includes("Copy original text preview"), false);
    assert.equal(await copy("Copy body preview"), raw);
    // This original has an LF XSSI prefix and a CRLF ending. Monaco's model
    // normalization must preserve the semantic map back to its exact tokens.
    await sourceContextAt("exact");
    assert.equal(await copy("Copy as JSON"), "9.007199254740993e15");
    await sourceContextAt("~leaf");
    assert.equal(await copy("Copy JSON Pointer"), "/a~1b/~0leaf");
    const largeRaw =
        '{"lead":"native-large-preview","payload":"' +
        "x".repeat(1024 * 1024 + 64) +
        '","tail":42}',
      // Keep this HTTP body uncompressed so the editor allocation limit is
      // independent of the native content-decoding expansion-ratio guard.
      largeBytes = Buffer.from(largeRaw),
      largePath = join(directory, "body-source-large-fixture.tmcap"),
      largeUrl = "https://native-source.invalid/body-source-large-fixture";
    await writeFile(
      largePath,
      nativeFixture(
        [
          {
            kind: "request-head",
            payload: { boundary: "client-request", method: "GET", target: largeUrl, headers: [] },
          },
          {
            kind: "response-head",
            payload: {
              boundary: "client-response",
              status: 200,
              headers: [["Content-Type", "application/json; charset=utf-8"]].map(
                ([name, value]) => ({
                  name: Array.from(Buffer.from(name)),
                  value: Array.from(Buffer.from(value)),
                }),
              ),
            },
          },
          {
            kind: "body-segment",
            payload: {
              boundary: "client-response",
              byte_count: largeBytes.length,
              bytes: Array.from(largeBytes),
              truncated: false,
            },
          },
          { kind: "completed", payload: null },
        ],
        732,
      ),
    );
    await evaluate(`${traffic}.importPaths([${JSON.stringify(largePath)}])`);
    await evaluate(
      `(async()=>{const w=${traffic},page=await window.__TAURI_INTERNALS__.invoke('query_sessions',{query:{search:'native-source.invalid',limit:10}}),row=page.sessions.find(row=>row.url===${JSON.stringify(largeUrl)});if(!row)throw new Error('Large retained fixture was not imported');globalThis.__bodySourceSmoke.ids.push(row.id);if(!await w.revealSession(row.id))throw new Error('Large retained fixture could not be revealed');})()`,
    );
    await wait(
      `${traffic}.selectedDetail?.id===globalThis.__bodySourceSmoke.ids[1]`,
      "large retained fixture selection",
    );
    await evaluate(
      `(async()=>{const i=${inspector};i.bodySourceMode='source';i.bodySourceWrap=true;i.bodyLimit.value='2097152';i.bodyViewerControl.value='original-text';i.bodyViewerControl.dispatchEvent(new Event('change',{bubbles:true}));await i.inspectBody();i.$flushUpdates();})()`,
    );
    const largeState = await evaluate(`(async()=>{
      const deadline=Date.now()+10000;let i,v;
      while(Date.now()<deadline){i=${inspector};v=i.querySelector('body-source-viewer');if(v?.sourceEditorLimited&&i.bodySourceLimited&&!v.fallback.hidden)break;await new Promise(resolve=>setTimeout(resolve,25));}
      return {mode:i.mode,viewer:i.viewer,label:i.viewerLabel,limit:i.bodyLimit.value,loading:i.loading,notice:i.bodyNotice,
        source:!!v,limited:v?.sourceEditorLimited??null,parentLimited:i.bodySourceLimited,status:v?.sourceStatus??null,
        displayLength:v?.bodySourceText.length??null,originalLength:v?.preview?.originalText?.length??null,
        previewTruncated:v?.preview?.truncated??null,fallbackVisible:!!v?.fallback&&!v.fallback.hidden,
        selectedId:i.detail?.id,expectedId:globalThis.__bodySourceSmoke.ids[1]};
    })()`);
    assert.ok(
      largeState.source &&
        largeState.limited &&
        largeState.parentLimited &&
        largeState.fallbackVisible,
      "Large retained source did not reach the selectable fallback: " + JSON.stringify(largeState),
    );
    assert.equal(
      largeState.viewer,
      "original-text",
      "Large fallback did not retain the chosen Text viewer",
    );
    assert.equal(await evaluate(`${source}.preview.formatted`), false);
    assert.equal(
      await evaluate(`${source}.bodySourceText.includes(String.fromCharCode(10))`),
      false,
      "Large fallback fixture was reformatted instead of remaining minified",
    );
    assert.equal(largeState.displayLength, largeRaw.length);
    assert.equal(
      await evaluate(`${source}.editor===null&&${source}.model===null`),
      true,
      "Large preview allocated Monaco",
    );
    assert.equal(await evaluate(`${source}.preview.truncated`), false);
    assert.equal(await evaluate(`${source}.preview.originalText.length`), largeRaw.length);
    assert.match(await evaluate(`${source}.sourceStatus`), /1 MiB/);
    await pointer(await point(`${inspector}.querySelector('.body-options > summary')`));
    const findButton = `[...${inspector}.querySelectorAll('.body-options-panel button')].find(button=>button.textContent.trim()==='Find')`,
      wrapInput = `[...${inspector}.querySelectorAll('.body-options-panel label')].find(label=>label.textContent.trim()==='Wrap').querySelector('input')`;
    assert.equal(
      await evaluate(`(${findButton}).disabled`),
      true,
      "Large fallback Find remains enabled",
    );
    await pointer(await point(wrapInput));
    assert.equal(
      await evaluate(
        `${source}.fallback.hasAttribute('data-nowrap')&&getComputedStyle(${source}.fallback).whiteSpace==='pre'`,
      ),
      true,
    );
    await pointer(await point(`${inspector}.querySelector('.body-options > summary')`));
    await pointer(await point(viewButton("Fields")));
    await wait(
      `${fields}?.visualizerRows.length>0&&!${fields}.visualizerBusy`,
      "large JSON Fields",
    );
    assert.equal(
      await evaluate(`!${source}.querySelector('.source-status')?.getClientRects().length`),
      true,
      "A source-only syntax limit notice consumed the Fields viewport",
    );
    const largeTailId = await fieldId("tail");
    await selectField(largeTailId, true);
    await pointer(await point(showSource));
    await wait(
      `${source}.sourceMode==='source'&&!${source}.fallback.hidden`,
      "large Fields reveal to fallback",
    );
    const fallbackReveal = await evaluate(
      `(async()=>{const deadline=Date.now()+3000;let result;do{const v=${source},root=v.getRootNode(),selection=root.getSelection?.()??document.getSelection(),range=selection?.rangeCount?selection.getRangeAt(0):null,r=range?.getBoundingClientRect(),fallback=v.fallback,bounds=fallback.getBoundingClientRect(),content=v.querySelector('.source-content').getBoundingClientRect(),body=${inspector}.querySelector('.body-scroll').getBoundingClientRect(),viewport={left:Math.max(0,bounds.left+fallback.clientLeft,content.left,body.left),right:Math.min(innerWidth,bounds.left+fallback.clientLeft+fallback.clientWidth,content.right,body.right),top:Math.max(0,bounds.top+fallback.clientTop,content.top,body.top),bottom:Math.min(innerHeight,bounds.top+fallback.clientTop+fallback.clientHeight,content.bottom,body.bottom)};result={text:selection?.toString(),top:fallback.scrollTop,left:fallback.scrollLeft,focused:root.activeElement===fallback,visible:!!r&&r.left>=viewport.left-1&&r.right<=viewport.right+1&&r.top>=viewport.top-1&&r.bottom<=viewport.bottom+1,range:r?.toJSON(),viewport,model:v.model===null};if(result.text==='42'&&result.focused&&result.visible&&result.left>0)break;await new Promise(resolve=>requestAnimationFrame(resolve));}while(Date.now()<deadline);return result;})()`,
    );
    assert.deepEqual(
      {
        text: fallbackReveal.text,
        focused: fallbackReveal.focused,
        visible: fallbackReveal.visible,
        model: fallbackReveal.model,
      },
      { text: "42", focused: true, visible: true, model: true },
      "Fallback source selection/focus is outside its visible pane: " +
        JSON.stringify(fallbackReveal),
    );
    assert.ok(
      fallbackReveal.left > 0,
      "No-wrap minified fallback did not scroll horizontally to its selected token",
    );
    await key("F10", 121, 8);
    await wait(`${menu}.matches(':popover-open')`, "large fallback semantic keyboard menu");
    assert.equal(await copy("Copy JSON Pointer"), "/tail");
    await capture("large-800");
    return {
      retainedCompressedEvidence: true,
      highlightedReadonly: true,
      pointerSelectionCopy: true,
      sourceClipboardEvent: true,
      nativeClipboard: false,
      nativeClipboardReason:
        "Native DataObject preservation is unavailable; clipboard writes were intercepted.",
      originalTextExact: true,
      foldedOffscreenCopy: true,
      emptySelectionPreventsCopy: true,
      keyboardMenuFocus: true,
      compactCaretVisible: true,
      sourceSemanticCopy: true,
      fieldsSemanticCopy: true,
      fieldsPointerDoesNotSelect: true,
      fieldsCollapsedSubtree: true,
      fieldsSourceNavigation: true,
      sourceFieldsPressedState: true,
      hiddenSourceFieldsDisposed: true,
      decodedText: true,
      mixedLineEndingSourceMap: true,
      largeRetainedFallback: true,
      largeFieldsFallbackReveal: true,
      layouts,
    };
  } catch (error) {
    failure = error;
    throw error;
  } finally {
    await call("Emulation.clearDeviceMetricsOverride");
    try {
      if (saved)
        await evaluate(`(async()=>{
        const saved=globalThis.__bodySourceSmoke;if(!saved)return;
        document.querySelector('inspection-copy-menu')?.close(false);
        const w=${traffic};
        if(saved.ids.length)await window.__TAURI_INTERNALS__.invoke('remove_traffic_entries',{ids:saved.ids,restore:false});
        w.queryRevision++;await w.refreshSessions(undefined,true);
        const remaining=await window.__TAURI_INTERNALS__.invoke('query_sessions',{query:{limit:1}});
        if(remaining.totalMatched!==saved.count)throw new Error('Native body fixture cleanup changed the retained catalog');
        if(saved.selectedId&&!await w.revealSession(saved.selectedId))throw new Error('Native body fixture cleanup could not restore the previous request');
        w.showInspectorSide(saved.side);if(w.inspectorExpanded!==saved.expanded)w.toggleInspector();
        const root=document.querySelector('app-shell').shadowRoot;(root.getSelection?.()??document.getSelection())?.removeAllRanges();
        const i=${inspector};i.viewer=saved.viewer;i.decodeContent=saved.decode;i.bodySourceMode=saved.sourceMode;i.bodySourceWrap=saved.sourceWrap;i.bodyLimit.value=saved.limit;i.bodyOptions.open=saved.optionsOpen;i.showMode(saved.mode);
        if(['body','split'].includes(saved.mode))await i.inspectBody();
        w.importStatus=saved.importStatus;w.$flushUpdates();
        if(saved.focus?.isConnected&&saved.focus.getClientRects().length)saved.focus.focus({preventScroll:true});
        const deadline=Date.now()+3000;while(w.totalMatched!==saved.count&&Date.now()<deadline){await new Promise(resolve=>setTimeout(resolve,25));await w.refreshSessions(undefined,true);}
        if(w.totalMatched!==saved.count)throw new Error('Native body fixture cleanup did not restore the Traffic count');
        delete globalThis.__bodySourceSmoke;
      })()`);
    } catch (cleanupError) {
      if (failure)
        throw new AggregateError(
          [failure, cleanupError],
          "Native source verification and cleanup failed",
        );
      throw cleanupError;
    } finally {
      await clipboard.restore();
    }
  }
}
