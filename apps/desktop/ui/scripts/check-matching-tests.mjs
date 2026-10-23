import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkMatchingSelectionRace(page, otherRuleId) {
  const previous = await page.evaluate(() => {
    const s = globalThis.__workspaceFixture,
      w = document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace");
    s.deferNextMatcher = true;
    s.matcherPending = false;
    w.matchTestUrl.value = "https://api.example.test/users/42";
    globalThis.__pendingMatcher = w.testMatcher();
    return w.editingAutoResponseId;
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.matcherPending);
  await page.locator("#rule-" + otherRuleId).click();
  await page.waitForFunction((id) => {
    const w = document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace");
    return w.editingAutoResponseId === id && !w.loadingSavedResponse;
  }, otherRuleId);
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
          .matcherTestBusy,
    ),
    false,
    "Selecting another rule left its matching controls locked",
  );
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.releaseMatcher();
    await globalThis.__pendingMatcher;
  });
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
          .matchTestResult,
    ),
    null,
    "A pending test from the previous rule appeared on the new selection",
  );
  await page.locator("#rule-" + previous).click();
  await page.waitForFunction((id) => {
    const w = document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace");
    return w.editingAutoResponseId === id && !w.loadingSavedResponse;
  }, previous);
}

export async function checkMatchingTests(page, root) {
  const form = page.locator(".auto-response-editor"),
    tester = form.locator(".matcher-test");
  const original = await page.evaluate(() => {
    const w = document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace");
    return {
      examples: structuredClone(w.matchExamples),
      url: w.matchTestUrl.value,
      headers: w.matchTestHeaders.value,
      method: w.autoResponseMethod.value,
      dirty: w.draftDirty,
    };
  });
  const check = async (url, expected) => {
    await tester.getByLabel("Test URL", { exact: true }).fill(url);
    await tester.getByRole("button", { name: "Check match", exact: true }).click();
    await tester
      .locator(".match-outcome")
      .getByText(expected ? "Matches" : "Does not match", { exact: true })
      .waitFor();
    await page.waitForFunction(
      () =>
        !document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
          .matcherTestBusy,
    );
  };
  try {
    for (const actual of [true, false])
      for (const expected of [true, false]) {
        const url = "https://api.example.test/users/" + (actual ? "43" : "letters");
        await check(url, actual);
        await tester
          .getByLabel("Expected result", { exact: true })
          .selectOption(expected ? "match" : "nonmatch");
        await tester.getByRole("button", { name: "Add test case", exact: true }).click();
        const row = tester
          .locator(".match-case")
          .filter({ has: page.getByText("GET " + url, { exact: true }) });
        await row
          .locator(".match-case-outcomes")
          .getByText(actual === expected ? "Passed" : "Failed", { exact: true })
          .waitFor();
        assert.deepEqual(await row.locator("dd").allTextContents(), [
          expected ? "Matches" : "Does not match",
          actual ? "Matches" : "Does not match",
          actual === expected ? "Passed" : "Failed",
        ]);
        assert.equal(
          await tester.locator(".match-outcome").textContent(),
          actual ? "Matches" : "Does not match",
          "Adding an expectation changed actual matching",
        );
      }
    await tester.locator(".match-test-headers > summary").click();
    for (const value of ["alpha", "beta"]) {
      await tester.locator(".match-test-headers textarea").fill("X-Variant: " + value);
      await check("https://api.example.test/users/44", true);
      await tester.getByRole("button", { name: "Add test case", exact: true }).click();
      await page.waitForFunction(
        () =>
          !document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
            .matcherTestBusy,
      );
    }
    const variants = tester
      .locator(".match-case")
      .filter({ has: page.getByText("GET https://api.example.test/users/44", { exact: true }) });
    assert.equal(await variants.count(), 2, "Header variants replaced one another");
    await variants.first().getByRole("button", { name: "Remove test case", exact: true }).click();
    assert.equal(await variants.count(), 1, "Removing a header variant removed another case");
    assert.match(await variants.locator("pre").textContent(), /beta/);
    // Cases remain inspectable even with no current ad-hoc request.
    await tester.getByLabel("Test URL", { exact: true }).fill("");
    await tester.getByRole("button", { name: "Check match", exact: true }).click();
    await page.waitForFunction(
      () =>
        !document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
          .matcherTestBusy,
    );
    assert.equal(await variants.locator("dd").last().textContent(), "Passed");
    await check("https://api.example.test/users/44", true);
    // A method edit immediately invalidates in-flight results. Let the new test
    // finish first, then deliver the old result to exercise the completion race.
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture,
        w = document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace");
      s.deferNextMatcher = true;
      s.matcherPending = false;
      globalThis.__pendingMatcher = w.testMatcher();
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.matcherPending);
    await form.locator('[name="method"]').selectOption("POST");
    await tester.locator(".match-outcome").getByText("Does not match", { exact: true }).waitFor();
    await page.evaluate(async () => {
      globalThis.__workspaceFixture.releaseMatcher();
      await globalThis.__pendingMatcher;
    });
    assert.equal(
      await tester.locator(".match-outcome").textContent(),
      "Does not match",
      "A late result replaced the newer method check",
    );
    await form.locator('[name="method"]').selectOption(original.method);
    await check("https://api.example.test/users/44", true);
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      await tester.locator(".match-test-result").scrollIntoViewIfNeeded();
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/matching-tests-${width}.png`),
      });
      assert.ok(
        await tester.getByRole("button", { name: "Add test case", exact: true }).isVisible(),
      );
    }
    await page.emulateMedia({ forcedColors: "active" });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/matching-tests-contrast.png"),
    });
    await page.emulateMedia({ forcedColors: "none", colorScheme: "dark" });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/matching-tests-dark.png"),
    });
    await page.emulateMedia({ colorScheme: "light" });
    await page.evaluate(() => {
      const w = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("automation-workspace");
      w.matchExamples = Array.from({ length: 32 }, (_, i) => ({
        method: "GET",
        url: "https://api.example.test/users/" + (100 + i),
        expected: true,
        headers: [],
      }));
    });
    await check("https://api.example.test/users/999", true);
    await tester.getByRole("button", { name: "Add test case", exact: true }).click();
    await tester
      .getByText("This rule has 32 test cases. Remove a case before adding another.", {
        exact: true,
      })
      .waitFor();
  } finally {
    await page.emulateMedia({ forcedColors: "none", colorScheme: "light" });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.evaluate(async (original) => {
      const w = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("automation-workspace");
      globalThis.__workspaceFixture.releaseMatcher?.();
      globalThis.__workspaceFixture.deferNextMatcher = false;
      w.autoResponseMethod.value = original.method;
      w.matchExamples = original.examples;
      w.matchTestUrl.value = original.url;
      w.matchTestHeaders.value = original.headers;
      w.draftDirty = original.dirty;
      w.matchExpectationChosen = false;
      await w.testMatcher();
    }, original);
  }
}
