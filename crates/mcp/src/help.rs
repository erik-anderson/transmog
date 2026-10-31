//! Trusted static guidance; no desktop, credential or captured content is involved.
use serde_json::{Value, json};

pub(crate) fn document() -> Value {
    serde_json::from_str(include_str!("../../proxy-script/help.json")).expect("bundled script help")
}

pub(crate) fn section_names() -> Vec<String> {
    let mut names = vec!["all".into(), "types".into()];
    names.extend(
        document()["sections"]
            .as_array()
            .expect("help sections")
            .iter()
            .map(|section| section["id"].as_str().expect("help section id").to_owned()),
    );
    names
}

pub(crate) fn read(input: &Value) -> Value {
    let mut help = document();
    let section = input["section"].as_str().unwrap_or("overview");
    let sections = help["sections"].as_array_mut().expect("help sections");
    if section == "types" {
        *sections = vec![json!({"id":"types","title":"TypeScript declarations",
            "paragraphs":[],"phases":[],"examples":[{"title":"transmog:api", "settings":"The installed sandbox contract; only this module may be imported.",
            "code":include_str!("../../proxy-script/generated/transmog-script.d.ts")} ]})];
    } else if section != "all" {
        sections.retain(|item| item["id"] == section);
    }
    help
}
