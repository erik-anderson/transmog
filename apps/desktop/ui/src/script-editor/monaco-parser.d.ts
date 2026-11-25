// Monaco bundles the parser implementation without declarations for its AST.
export interface AstNode {
  readonly pos: number;
  readonly end: number;
  readonly name?: AstNode;
  getStart(file: SourceFile): number;
}

interface SourceFile {
  readonly statements: readonly AstNode[];
  readonly parseDiagnostics: readonly { readonly start: number }[];
}

interface Identifier extends AstNode {
  readonly text: string;
}

export type BindingName = Identifier | (AstNode & { readonly elements: readonly AstNode[] });

interface BindingNode extends AstNode {
  readonly name: BindingName;
}

interface VariableDeclarationList extends AstNode {
  readonly flags: number;
  readonly declarations: readonly BindingNode[];
}

interface NamedNode extends AstNode {
  readonly name: Identifier;
}

interface NamedImports extends AstNode {
  readonly elements: readonly {
    readonly name: Identifier;
    readonly propertyName?: Identifier;
    readonly isTypeOnly: boolean;
  }[];
}

interface ImportDeclaration extends AstNode {
  readonly importClause?: {
    readonly name?: Identifier;
    readonly namedBindings?: NamedNode | NamedImports;
    readonly isTypeOnly: boolean;
  };
  readonly moduleSpecifier: AstNode;
}

interface NamedExports extends AstNode {
  readonly elements: readonly NamedNode[];
}

export interface ScriptParser {
  readonly ScriptTarget: { readonly Latest: number };
  readonly ScriptKind: { readonly JS: number; readonly TS: number };
  readonly NodeFlags: { readonly BlockScoped: number };
  readonly SyntaxKind: {
    readonly ExportKeyword: number;
    readonly AsyncKeyword: number;
    readonly DeclareKeyword: number;
    readonly DefaultKeyword: number;
  };
  createSourceFile(
    name: string,
    source: string,
    target: number,
    setParentNodes: boolean,
    kind: number,
  ): SourceFile;
  forEachChild(node: AstNode, visit: (child: AstNode) => void): void;
  isIdentifier(node: AstNode): node is Identifier;
  isBindingElement(node: AstNode): node is BindingNode;
  isFunctionLike(node: AstNode): boolean;
  isClassLike(node: AstNode): boolean;
  isModuleDeclaration(node: AstNode): boolean;
  isVariableDeclarationList(node: AstNode): node is VariableDeclarationList;
  isVariableStatement(node: AstNode): node is AstNode & {
    readonly declarationList: VariableDeclarationList;
  };
  isImportDeclaration(node: AstNode): node is ImportDeclaration;
  isNamespaceImport(node: AstNode): node is NamedNode;
  isStringLiteral(node: AstNode): node is AstNode & { readonly text: string };
  isExportDeclaration(node: AstNode): node is AstNode & {
    readonly exportClause?: NamedExports | NamedNode;
  };
  isNamedExports(node: AstNode): node is NamedExports;
  isFunctionDeclaration(node: AstNode): node is AstNode & {
    readonly body?: AstNode;
    readonly asteriskToken?: AstNode;
  };
  canHaveModifiers(node: AstNode): boolean;
  getModifiers(node: AstNode): readonly { readonly kind: number }[] | undefined;
}
