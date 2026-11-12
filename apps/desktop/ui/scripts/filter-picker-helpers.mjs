export async function fillFilterValues(picker, value) {
  for (const selector of [".filter-custom", ".filter-authoring"]) {
    const details = picker.locator(selector);
    if (!(await details.evaluate((element) => element.open))) {
      await details.locator(":scope > summary").click();
    }
  }
  await picker.getByLabel("Is in", { exact: true }).fill(value);
}
