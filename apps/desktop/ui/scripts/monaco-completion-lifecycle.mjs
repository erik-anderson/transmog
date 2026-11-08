import { readFile } from "node:fs/promises";

// Monaco 0.57's shared JSON/CSS/HTML completion adapter reads the model after
// awaiting its worker. Editors still dispose their owned models on close.
export function monacoCompletionLifecycle() {
  return {
    name: "monaco-completion-lifecycle",
    setup(build) {
      let patched = false;
      build.onLoad(
        {
          filter:
            /[/\\]monaco-editor[/\\]esm[/\\]vs[/\\]languages[/\\]features[/\\]common[/\\]lspLanguageFeatures\.js$/,
        },
        async ({ path }) => {
          let contents = await readFile(path, "utf8");
          for (const [before, after] of [
            [
              "return worker.doComplete(resource.toString(), fromPosition(position));",
              "if (token.isCancellationRequested || model.isDisposed()) return;\n      return worker.doComplete(resource.toString(), fromPosition(position));",
            ],
            [
              "if (!info) {\n        return;\n      }\n      const wordInfo = model.getWordUntilPosition(position);",
              "if (!info || token.isCancellationRequested || model.isDisposed()) {\n        return;\n      }\n      const wordInfo = model.getWordUntilPosition(position);",
            ],
          ]) {
            if (contents.split(before).length !== 2)
              throw new Error("Review Monaco's completion lifecycle adapter before bundling it.");
            contents = contents.replace(before, after);
          }
          patched = true;
          return { contents, loader: "js" };
        },
      );
      build.onEnd(() => {
        if (!patched) throw new Error("Monaco's completion lifecycle adapter was not bundled.");
      });
    },
  };
}
