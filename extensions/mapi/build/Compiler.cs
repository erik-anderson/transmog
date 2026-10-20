// Developer-only static compiler; no CLR, compiler or source text execution ships in tmext.
using System.Text.Json;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Transpose.Compiler.Library;

if (args.Length != 3) throw new ArgumentException("Expected source inventory, output JavaScript and pinned BCL assembly paths");
Environment.SetEnvironmentVariable("TRANSPOSE_DLL_PATH", Path.GetFullPath(args[2]));
var inputs = JsonSerializer.Deserialize<Dictionary<string, string>>(File.ReadAllText(args[0]))!;
var request = new CompilationRequest("TransmogMapi").WithRuntime();
var enums = new SortedDictionary<string, string>(StringComparer.Ordinal);
var changes = new SortedDictionary<string, int>(StringComparer.Ordinal);
var trees = new List<SyntaxTree>();
foreach (var (name, source) in inputs.OrderBy(item => item.Key, StringComparer.Ordinal))
{
    var tree = CSharpSyntaxTree.ParseText(source);
    var root = tree.GetRoot();
    foreach (var declaration in root.DescendantNodes().OfType<EnumDeclarationSyntax>())
    {
        var path = declaration.Ancestors().Reverse().Select(node => node switch
        {
            BaseNamespaceDeclarationSyntax ns => ns.Name.ToString(),
            TypeDeclarationSyntax owner => owner.Identifier.ValueText,
            _ => null
        }).Where(value => value != null).Append(declaration.Identifier.ValueText);
        enums.Add(string.Join('.', path), declaration.BaseList?.Types.Single().Type.ToString() ?? "int");
    }
    var rewriter = new PortableSource(name, changes);
    trees.Add(CSharpSyntaxTree.ParseText(rewriter.Visit(root)!.ToFullString(), path: name));
}
var enumSource = "using System; namespace BlockParser { public static partial class PortablePrimitives { public static Type Underlying(Type type) {\n"
    + string.Join('\n', enums.Select(item => $"if(type==typeof({item.Key}))return typeof({item.Value});"))
    + "\nreturn type; } } }";
trees.Add(CSharpSyntaxTree.ParseText(enumSource, path: "EnumUnderlying.cs"));
// The emitter misses some user-defined implicit conversions in comparisons,
// switch governing expressions, array bounds and arithmetic. Resolve them with
// Roslyn against the same pinned BCL before emitting ordinary method calls.
var semantic = CSharpCompilation.Create("TransmogMapiSemantics", trees,
    [MetadataReference.CreateFromFile(args[2])], new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary, allowUnsafe: true));
var errors = semantic.GetDiagnostics().Where(item => item.Severity == DiagnosticSeverity.Error).ToArray();
if (errors.Length > 0)
{
    foreach (var error in errors) Console.Error.WriteLine(error);
    return 1;
}
foreach (var tree in trees)
{
    var converted = new BlockConversions(semantic.GetSemanticModel(tree), changes).Visit(tree.GetRoot())!;
    request = request.WithSourceFile(tree.FilePath, converted.ToFullString());
}
var result = TransposeCompilerLibrary.Compile(request);
if (!result.Success)
{
    foreach (var error in result.Errors) Console.Error.WriteLine(error);
    return 1;
}
File.WriteAllText(args[1], result.Javascript);
Console.WriteLine(JsonSerializer.Serialize(new { enums = enums.Count, adaptations = changes }));
return 0;

sealed class PortableSource(string file, SortedDictionary<string, int> changes) : CSharpSyntaxRewriter
{
    bool HasPayload => file is "Parsers/MSOXCMAPIHTTP/structs/ExtendedBuffer.cs"
        or "Parsers/MSOXCMAPIHTTP/structs/ExtendedBuffer_Input.cs"
        or "Parsers/MSOXCMAPIHTTP/structs/RgbOutputBuffer.cs";
    void Count(string change)
    {
        changes.TryGetValue(change, out var count);
        changes[change] = count + 1;
    }
    public override SyntaxNode? VisitUsingDirective(UsingDirectiveSyntax node)
    {
        if (node.Name?.ToString() is "System.Net.Mail" or "System.Security.Cryptography")
        {
            Count("unused namespace");
            return null;
        }
        return base.VisitUsingDirective(node);
    }
    public override SyntaxNode? VisitSizeOfExpression(SizeOfExpressionSyntax node)
    {
        var size = node.Type.ToString() switch
        {
            "byte" or "RopIdType" or "RestrictTypeEnum" => 1,
            "char" or "ushort" => 2,
            "uint" => 4,
            _ => throw new InvalidOperationException("Unqualified sizeof adaptation: " + node)
        };
        Count("fixed sizeof");
        return SyntaxFactory.LiteralExpression(SyntaxKind.NumericLiteralExpression, SyntaxFactory.Literal(size)).WithTriviaFrom(node);
    }
    public override SyntaxNode? VisitMethodDeclaration(MethodDeclarationSyntax node)
    {
        if (file == "BlockParser/BinaryParser.cs")
        {
            string? body = node.Identifier.ValueText switch
            {
                "CheckSize" => "{return cb >= 0 && Offset >= 0 && Offset <= size && cb <= size - Offset;}",
                "Advance" => "{long next=(long)Offset+cb; if(next<0 || next>size)throw new ArgumentOutOfRangeException(nameof(cb)); Offset=(int)next;}",
                "PushCap" => "{if(cap<0 || !CheckSize(cap))throw new ArgumentOutOfRangeException(nameof(cap)); sizes.Push(size); if(cap!=0)size=Offset+cap;}",
                _ => null
            };
            if (body != null)
            {
                Count("bounded binary reader");
                return node.WithExpressionBody(null).WithSemicolonToken(default).WithBody((BlockSyntax)SyntaxFactory.ParseStatement(body));
            }
        }
        if (file == "BlockParser/BlockT.cs" && node.Identifier.ValueText == "ReadStruct")
        {
            Count("managed primitive read");
            return node.WithBody((BlockSyntax)SyntaxFactory.ParseStatement("{return (U)PortablePrimitives.Read(typeof(U), parser.ReadBytes(PortablePrimitives.SizeOf(typeof(U))));}"));
        }
        return base.VisitMethodDeclaration(node);
    }
    public override SyntaxNode? VisitLocalDeclarationStatement(LocalDeclarationStatementSyntax node)
    {
        if (file == "BlockParser/Strings.cs" && node.Declaration.Variables.Count == 1 && node.Declaration.Variables[0].Identifier.ValueText == "encoding")
        {
            Count("ASCII replacement fallback");
            return null;
        }
        return base.VisitLocalDeclarationStatement(node);
    }
    public override SyntaxNode? VisitInvocationExpression(InvocationExpressionSyntax node)
    {
        if (HasPayload && node.Expression is MemberAccessExpressionSyntax member && member.Name.Identifier.ValueText == "ShiftOffset")
        {
            Count("derived source coordinates");
            return SyntaxFactory.ParseExpression($"PortableEvidence.Shift({member.Expression}, {node.ArgumentList.Arguments.Single()}, isCompressedXOR)").WithTriviaFrom(node);
        }
        var expression = node.Expression.ToString();
        if (file.StartsWith("Parsers/", StringComparison.Ordinal) && expression == "parser.PushCap")
        {
            Count("strict protocol sub-buffer");
            return ((InvocationExpressionSyntax)base.VisitInvocationExpression(node)!).WithExpression(SyntaxFactory.ParseExpression("parser.PushStrictCap"));
        }
        string? replacement = null;
        if (file is "BlockParser/BlockT.cs" or "BlockParser/BlockParsers.cs")
        {
            replacement = expression switch
            {
                "Enum.GetUnderlyingType" => "PortablePrimitives.Underlying",
                "System.Runtime.InteropServices.Marshal.SizeOf" => "PortablePrimitives.SizeOf",
                _ => null
            };
        }
        if (file == "BlockParser/BinaryParser.cs" && expression == "Buffer.BlockCopy") replacement = "Array.Copy";
        if (file == "BlockParser/Strings.cs" && expression == "encoding.GetString") replacement = "PortablePrimitives.Ascii";
        if (replacement != null)
        {
            Count(expression);
            return node.WithExpression(SyntaxFactory.ParseExpression(replacement));
        }
        return base.VisitInvocationExpression(node);
    }
    public override SyntaxNode? VisitObjectCreationExpression(ObjectCreationExpressionSyntax node)
    {
        if (HasPayload && node.Type.ToString() == "BinaryParser")
        {
            Count("payload source provenance");
            return SyntaxFactory.ParseExpression("PortableEvidence.Payload(payloadBytes, payloadBlock, isCompressedXOR, (uint)RPCHEADEREXT.Flags.Data)").WithTriviaFrom(node);
        }
        return base.VisitObjectCreationExpression(node);
    }
    public override SyntaxNode? VisitClassDeclaration(ClassDeclarationSyntax node)
    {
        var visited = (ClassDeclarationSyntax)base.VisitClassDeclaration(node)!;
        if (file != "BlockParser/BinaryParser.cs" || node.Identifier.ValueText != "BinaryParser") return visited;
        return visited.AddMembers(SyntaxFactory.ParseMemberDeclaration("public void PushStrictCap(int cap) { if(!CheckSize(cap))throw new ArgumentOutOfRangeException(nameof(cap)); sizes.Push(size); size=Offset+cap; }")!);
    }
}

sealed class BlockConversions(SemanticModel model, SortedDictionary<string, int> changes) : CSharpSyntaxRewriter
{
    public override SyntaxNode? VisitInterpolation(InterpolationSyntax node)
    {
        var visited = (InterpolationSyntax)base.VisitInterpolation(node)!;
        if (node.FormatClause?.FormatStringToken.ValueText != "X" || model.GetTypeInfo(node.Expression).Type is not INamedTypeSymbol { TypeKind: TypeKind.Enum } type) return visited;
        var digits = type.EnumUnderlyingType!.SpecialType switch
        {
            SpecialType.System_Byte or SpecialType.System_SByte => 2,
            SpecialType.System_Int16 or SpecialType.System_UInt16 => 4,
            SpecialType.System_Int32 or SpecialType.System_UInt32 => 8,
            SpecialType.System_Int64 or SpecialType.System_UInt64 => 16,
            _ => throw new InvalidOperationException("Invalid enum width")
        };
        changes.TryGetValue("enum hexadecimal width", out var count);
        changes["enum hexadecimal width"] = count + 1;
        var unsigned = digits switch { 2 => "byte", 4 => "ushort", 8 => "uint", _ => "ulong" };
        return visited.WithExpression(SyntaxFactory.ParseExpression($"BlockParser.PortablePrimitives.EnumHex(unchecked(({unsigned})({visited.Expression})), {digits})"))
            .WithFormatClause(null);
    }
    public override SyntaxNode? Visit(SyntaxNode? node)
    {
        var visited = base.Visit(node);
        if (node is not ExpressionSyntax expression || visited is not ExpressionSyntax value) return visited;
        // Roslyn also reports the bound member conversion on its name token;
        // only the complete member expression can be replaced with a call.
        if (expression.Parent is MemberAccessExpressionSyntax member && member.Name == expression
            || expression.Parent is MemberBindingExpressionSyntax binding && binding.Name == expression
            || expression.Parent is QualifiedNameSyntax or AliasQualifiedNameSyntax) return visited;
        var conversion = model.GetConversion(expression);
        if (!conversion.IsUserDefined || conversion.MethodSymbol is not { } method || method.ContainingNamespace.ToString() != "BlockParser") return visited;
        string helper;
        if (method.ContainingType.Name == "BlockT") helper = method.ReturnType.SpecialType == SpecialType.System_String ? "Text" : "Value";
        else if (method.ContainingType.Name == "BlockString") helper = "Text";
        else throw new InvalidOperationException("Unqualified parser conversion: " + method);
        changes.TryGetValue("explicit parser conversion", out var count);
        changes["explicit parser conversion"] = count + 1;
        return SyntaxFactory.InvocationExpression(SyntaxFactory.ParseExpression("BlockParser.PortablePrimitives." + helper),
            SyntaxFactory.ArgumentList(SyntaxFactory.SingletonSeparatedList(SyntaxFactory.Argument(value.WithoutTrivia())))).WithTriviaFrom(node);
    }
}
