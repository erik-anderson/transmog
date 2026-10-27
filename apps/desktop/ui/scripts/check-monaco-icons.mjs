import assert from "node:assert/strict";

/** Check the font even when a one-line editor or diff has no gutter icons. */
export async function checkMonacoIcons(host) {
  const rendered = await host.evaluate(async (element) => {
    const icon = element.querySelector(".codicon");
    const style = icon ? getComputedStyle(icon, "::before") : null;
    const faces = await document.fonts.load("16px codicon", style?.content ?? "\ueab4");
    return {
      family: style?.fontFamily,
      content: style?.content,
      faces: faces.map((face) => ({ family: face.family, status: face.status })),
    };
  });
  if (rendered.family !== undefined) {
    assert.match(rendered.family, /codicon/);
    assert.ok(rendered.content && rendered.content !== "none");
  }
  assert.ok(
    rendered.faces.some((face) => face.family === "codicon" && face.status === "loaded"),
    "Monaco icon font is unavailable to the document; icons render as missing-glyph boxes",
  );
}
