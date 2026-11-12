/** Edit through the bulk-text surface so checks exercise the same change event as typing. */
export async function fillHeaderEditor(editor, value) {
  const textButton = editor.getByRole("button", { name: "Edit as text", exact: true });
  if (await textButton.isVisible()) {
    await textButton.click();
  }
  await editor.getByRole("textbox", { name: / as text$/ }).fill(value);
}
