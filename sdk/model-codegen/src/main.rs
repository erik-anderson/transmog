//! Extract the canonical Rust declarations used by the SDK projection generator.
//! This tool deliberately uses Rust's syntax tree rather than a second schema.
use quote::ToTokens;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
use syn::{Fields, Item, Type, UseTree, Visibility};

fn ty(value: &Type) -> Value {
    match value {
        Type::Path(value) if value.qself.is_none() => {
            let path = value
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>();
            let arguments = &value.path.segments.last().unwrap().arguments;
            let types = match arguments {
                syn::PathArguments::AngleBracketed(arguments) => arguments
                    .args
                    .iter()
                    .filter_map(|argument| {
                        if let syn::GenericArgument::Type(t) = argument {
                            Some(ty(t))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>(),
                _ => vec![],
            };
            json!({"path": path, "args": arguments.to_token_stream().to_string(), "types": types})
        }
        Type::Reference(value) => json!({"reference":ty(&value.elem)}),
        Type::Array(value) => {
            json!({"array":ty(&value.elem),"length":value.len.to_token_stream().to_string()})
        }
        Type::Slice(value) => json!({"slice":ty(&value.elem)}),
        Type::Tuple(value) => json!({"tuple":value.elems.iter().map(ty).collect::<Vec<_>>()}),
        _ => json!({"unsupported":value.to_token_stream().to_string()}),
    }
}
fn attribute_text(attribute: &syn::Attribute, name: &str) -> Option<String> {
    if attribute.path().is_ident(name)
        && let syn::Meta::NameValue(value) = &attribute.meta
        && let syn::Expr::Lit(literal) = &value.value
        && let syn::Lit::Str(text) = &literal.lit
    {
        Some(text.value())
    } else {
        None
    }
}

fn conditional(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().is_ident("cfg") || a.path().is_ident("cfg_attr"))
}

fn docs(attrs: &[syn::Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter_map(|a| attribute_text(a, "doc").map(|text| text.trim().to_string()))
        .collect()
}
fn fields(value: &Fields) -> Vec<Value> {
    value.iter().enumerate().map(|(index, field)| json!({
        "name": field.ident.as_ref().map(ToString::to_string).unwrap_or_else(|| index.to_string()),
        "type": ty(&field.ty), "public": matches!(field.vis, Visibility::Public(_)),
        "conditional": conditional(&field.attrs), "docs": docs(&field.attrs)
    })).collect()
}
fn imports(tree: &UseTree, prefix: &[String], public: bool, values: &mut Vec<Value>) {
    match tree {
        UseTree::Path(p) => {
            let mut path = prefix.to_vec();
            path.push(p.ident.to_string());
            imports(&p.tree, &path, public, values);
        }
        UseTree::Name(n) => {
            let mut path = prefix.to_vec();
            if n.ident != "self" {
                path.push(n.ident.to_string());
            }
            values.push(json!({"name":path.last(),"path":path,"public":public}));
        }
        UseTree::Rename(n) => {
            let mut path = prefix.to_vec();
            path.push(n.ident.to_string());
            values.push(json!({"name":n.rename.to_string(),"path":path,"public":public}));
        }
        UseTree::Glob(_) => values.push(json!({"name":"*","path":prefix,"public":public})),
        UseTree::Group(g) => {
            for item in &g.items {
                imports(item, prefix, public, values)
            }
        }
    }
}
fn scan(
    items: &[Item],
    module: &str,
    directory: &Path,
    public: bool,
    modules: &mut BTreeMap<String, Value>,
    types: &mut BTreeMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut uses = vec![];
    for item in items {
        match item {
            Item::Use(u) => imports(
                &u.tree,
                &[],
                matches!(u.vis, Visibility::Public(_)),
                &mut uses,
            ),
            Item::Struct(s) => {
                types.insert(format!("{module}::{}", s.ident), json!({
                    "name": s.ident.to_string(), "module": module, "kind": "struct",
                    "conditional": conditional(&s.attrs), "generic": !s.generics.params.is_empty(),
                    "public": matches!(s.vis, Visibility::Public(_)), "fields": fields(&s.fields),
                    "tuple": matches!(s.fields, Fields::Unnamed(_)), "docs": docs(&s.attrs)
                }));
            }
            Item::Enum(e) => {
                let variants = e.variants.iter().map(|v| json!({
                    "name": v.ident.to_string(), "conditional": conditional(&v.attrs),
                    "fields": fields(&v.fields), "tuple": matches!(v.fields, Fields::Unnamed(_)),
                    "docs": docs(&v.attrs)
                })).collect::<Vec<_>>();
                types.insert(format!("{module}::{}", e.ident), json!({
                    "name": e.ident.to_string(), "module": module, "kind": "enum",
                    "conditional": conditional(&e.attrs), "generic": !e.generics.params.is_empty(),
                    "public": matches!(e.vis, Visibility::Public(_)), "variants": variants,
                    "docs": docs(&e.attrs)
                }));
            }
            Item::Type(t) => {
                types.insert(format!("{module}::{}", t.ident), json!({
                    "name": t.ident.to_string(), "module": module, "kind": "alias",
                    "conditional": conditional(&t.attrs), "generic": !t.generics.params.is_empty(),
                    "public": matches!(t.vis, Visibility::Public(_)), "type": ty(&t.ty), "docs": docs(&t.attrs)
                }));
            }
            Item::Mod(m)
                if m.ident != "tests"
                    && !m.attrs.iter().any(|a| {
                        a.path().is_ident("cfg")
                            && a.meta.to_token_stream().to_string().contains("test")
                    }) =>
            {
                let name = m.ident.to_string();
                let next = format!("{module}::{name}");
                let visible = public && matches!(m.vis, Visibility::Public(_));
                if let Some((_, items)) = &m.content {
                    scan(
                        items,
                        &next,
                        &directory.join(&name),
                        visible,
                        modules,
                        types,
                    )?;
                } else {
                    let direct = directory.join(format!("{name}.rs"));
                    let nested = directory.join(&name).join("mod.rs");
                    let path = m
                        .attrs
                        .iter()
                        .find_map(|a| attribute_text(a, "path").map(|path| directory.join(path)))
                        .unwrap_or(if direct.exists() { direct } else { nested });
                    if path.exists() {
                        let file = syn::parse_file(&std::fs::read_to_string(&path)?)?;
                        scan(
                            &file.items,
                            &next,
                            &directory.join(&name),
                            visible,
                            modules,
                            types,
                        )?;
                    }
                }
            }
            _ => {}
        }
    }
    modules.insert(module.into(), json!({"public":public,"imports":uses}));
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut modules = BTreeMap::new();
    let mut types = BTreeMap::new();
    for argument in std::env::args().skip(1) {
        let (name, path) = argument.split_once('=').ok_or("Expected crate=lib.rs")?;
        let path = Path::new(path);
        let file = syn::parse_file(&std::fs::read_to_string(path)?)?;
        scan(
            &file.items,
            name,
            path.parent().unwrap(),
            true,
            &mut modules,
            &mut types,
        )?;
    }
    println!("{}", json!({"modules":modules,"types":types}));
    Ok(())
}
