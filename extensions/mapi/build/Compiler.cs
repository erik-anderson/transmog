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
    request = request.WithSourceFile(name, rewriter.Visit(root)!.ToFullString());
}
var enumSource = "using System; namespace BlockParser { public static partial class PortablePrimitives { public static Type Underlying(Type type) {\n"
    + string.Join('\n', enums.Select(item => $"if(type==typeof({item.Key}))return typeof({item.Value});"))
    + "\nreturn type; } } }";
request = request.WithSourceFile("EnumUnderlying.cs", enumSource);
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
        var expression = node.Expression.ToString();
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
}
