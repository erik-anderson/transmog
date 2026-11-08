import type * as TypeScript from "../../../../../node_modules/typescript/lib/typescript.js";
import { handlers, type ScriptAnalysis } from "./handlers.js";

/** Runs inside Monaco's TypeScript worker using its existing parser. */
export function analyzeScript(
  ts: typeof TypeScript,
  source: string,
  language: string,
): ScriptAnalysis {
  const file = ts.createSourceFile(
    "main." + (language === "javascript" ? "js" : "ts"),
    source,
    ts.ScriptTarget.Latest,
    true,
    language === "javascript" ? ts.ScriptKind.JS : ts.ScriptKind.TS,
  );
  const declarations = new Map<string, TypeScript.Node[]>();
  const imports: ScriptAnalysis["imports"] = [];
  const bind = (name: string, node: TypeScript.Node) =>
    declarations.set(name, [...(declarations.get(name) ?? []), node]);
  const binding = (name: TypeScript.BindingName, node: TypeScript.Node) => {
    if (ts.isIdentifier(name)) {
      bind(name.text, node);
    } else {
      for (const element of name.elements) {
        if (ts.isBindingElement(element)) {
          binding(element.name, node);
        }
      }
    }
  };
  const collectHoistedVariables = (node: TypeScript.Node): void => {
    if (ts.isFunctionLike(node) || ts.isClassLike(node) || ts.isModuleDeclaration(node)) {
      return;
    }
    if (ts.isVariableDeclarationList(node) && !(node.flags & ts.NodeFlags.BlockScoped)) {
      for (const declaration of node.declarations) {
        binding(declaration.name, node);
      }
      return;
    }
    ts.forEachChild(node, collectHoistedVariables);
  };
  for (const statement of file.statements) {
    if (!ts.isVariableStatement(statement)) {
      collectHoistedVariables(statement);
    }
    if (ts.isImportDeclaration(statement) && statement.importClause) {
      const clause = statement.importClause;
      if (clause.name) {
        bind(clause.name.text, statement);
      }
      const named = clause.namedBindings;
      if (named && ts.isNamespaceImport(named)) {
        bind(named.name.text, statement);
        if (
          ts.isStringLiteral(statement.moduleSpecifier) &&
          statement.moduleSpecifier.text === "transmog:api"
        ) {
          imports.push({ imported: "*", local: named.name.text, typeOnly: clause.isTypeOnly });
        }
      } else if (named) {
        for (const item of named.elements) {
          bind(item.name.text, statement);
          if (
            ts.isStringLiteral(statement.moduleSpecifier) &&
            statement.moduleSpecifier.text === "transmog:api"
          ) {
            imports.push({
              imported: item.propertyName?.text ?? item.name.text,
              local: item.name.text,
              typeOnly: clause.isTypeOnly || item.isTypeOnly,
            });
          }
        }
      }
    } else if (ts.isVariableStatement(statement)) {
      for (const declaration of statement.declarationList.declarations) {
        binding(declaration.name, statement);
      }
    } else if (
      "name" in statement &&
      statement.name &&
      ts.isIdentifier(statement.name as TypeScript.Node)
    ) {
      bind((statement.name as TypeScript.Identifier).text, statement);
    }
    // An export alias can occupy a handler's public name independently of its local name.
    if (ts.isExportDeclaration(statement) && statement.exportClause) {
      if (ts.isNamedExports(statement.exportClause)) {
        for (const element of statement.exportClause.elements) {
          bind(element.name.text, element);
        }
      } else {
        bind(statement.exportClause.name.text, statement.exportClause);
      }
    }
  }
  const diagnostics = (
    file as TypeScript.SourceFile & { parseDiagnostics: TypeScript.DiagnosticWithLocation[] }
  ).parseDiagnostics;
  const locations: ScriptAnalysis["locations"] = handlers.map((handler) => {
    const nodes = declarations.get(handler.name) ?? [];
    const implementations = nodes.filter((node) => ts.isFunctionDeclaration(node) && node.body);
    const node = implementations[0] ?? nodes[0];
    const modifiers = node && ts.canHaveModifiers(node) ? (ts.getModifiers(node) ?? []) : [];
    const valid =
      implementations.length === 1 &&
      nodes.every((item) => ts.isFunctionDeclaration(item)) &&
      node &&
      ts.isFunctionDeclaration(node) &&
      node.body &&
      !node.asteriskToken &&
      modifiers.some((item) => item.kind === ts.SyntaxKind.ExportKeyword) &&
      !modifiers.some((item) =>
        [
          ts.SyntaxKind.AsyncKeyword,
          ts.SyntaxKind.DeclareKeyword,
          ts.SyntaxKind.DefaultKeyword,
        ].includes(item.kind),
      ) &&
      !diagnostics.some((item) => item.start >= node.pos && item.start <= node.end);
    const name = node && "name" in node ? (node.name as TypeScript.Node | undefined) : undefined;
    const start = (name ?? node)?.getStart(file) ?? diagnostics[0]?.start ?? 0;
    return {
      name: handler.name,
      action: valid ? "Go to" : node || diagnostics.length ? "Review" : "Add",
      start,
      end: name?.end ?? start,
      reason: valid
        ? ""
        : node
          ? "Review " +
            handler.name +
            ". Use one directly exported synchronous function with a complete body."
          : diagnostics.length
            ? "Fix the source syntax before adding a handler."
            : "",
    };
  });
  return {
    handlers: locations.filter((item) => item.action === "Go to").map((item) => item.name),
    locations,
    bindings: [...declarations.keys()],
    imports,
  };
}
