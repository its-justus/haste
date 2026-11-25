//! `protowatch diff`: field-level differences of the watched messages between two
//! compiled sets of definitions.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{Result, bail};
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorSet};

/// Messages and enums of one set, by full name without the leading dot.
#[derive(Default)]
struct Index<'a> {
    messages: BTreeMap<String, &'a DescriptorProto>,
    enums: BTreeMap<String, &'a EnumDescriptorProto>,
}

impl<'a> Index<'a> {
    fn new(set: &'a FileDescriptorSet) -> Self {
        let mut index = Index::default();
        for file in &set.file {
            let prefix = file.package().to_owned();
            for m in &file.message_type {
                index.add_message(&prefix, m);
            }
            for e in &file.enum_type {
                index.enums.insert(join(&prefix, e.name()), e);
            }
        }
        index
    }

    fn add_message(&mut self, prefix: &str, m: &'a DescriptorProto) {
        let name = join(prefix, m.name());
        for nested in &m.nested_type {
            self.add_message(&name, nested);
        }
        for e in &m.enum_type {
            self.enums.insert(join(&name, e.name()), e);
        }
        self.messages.insert(name, m);
    }

    /// Names of the messages and enums the fields of `name` refer to.
    fn references(&self, name: &str) -> Vec<String> {
        let Some(m) = self.messages.get(name) else {
            return Vec::new();
        };
        m.field
            .iter()
            .filter(|f| matches!(f.r#type(), Type::Message | Type::Group | Type::Enum))
            .map(|f| f.type_name().trim_start_matches('.').to_owned())
            .collect()
    }
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}

/// What differs, per message or enum. Empty when nothing does.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// (type name, one line per difference), sorted by name.
    pub entries: Vec<(String, Vec<String>)>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        for (name, lines) in &self.entries {
            let _ = writeln!(out, "### `{name}`\n");
            for line in lines {
                let _ = writeln!(out, "- {line}");
            }
            out.push('\n');
        }
        out
    }
}

/// Compares `old` and `new` for every name in `watched` and every message and enum reachable
/// from them through fields (in either set). A watched name found in neither set is an error.
pub fn diff(
    old: &FileDescriptorSet,
    new: &FileDescriptorSet,
    watched: &[String],
) -> Result<Report> {
    let (a, b) = (Index::new(old), Index::new(new));

    let mut todo: Vec<String> = Vec::new();
    for name in watched {
        let known = [&a, &b]
            .iter()
            .any(|i| i.messages.contains_key(name) || i.enums.contains_key(name));
        if !known {
            bail!("WATCHED names `{name}`, which is in neither set of definitions");
        }
        todo.push(name.clone());
    }
    let mut seen = BTreeSet::new();
    while let Some(name) = todo.pop() {
        if seen.insert(name.clone()) {
            todo.extend(a.references(&name));
            todo.extend(b.references(&name));
        }
    }

    let mut report = Report::default();
    for name in &seen {
        let mut lines = Vec::new();
        match (a.messages.get(name), b.messages.get(name)) {
            (Some(x), Some(y)) => diff_message(x, y, &mut lines),
            (Some(_), None) if !b.enums.contains_key(name) => lines.push("message removed".into()),
            (None, Some(_)) if !a.enums.contains_key(name) => lines.push("message added".into()),
            _ => {}
        }
        match (a.enums.get(name), b.enums.get(name)) {
            (Some(x), Some(y)) => diff_enum(x, y, &mut lines),
            (Some(_), None) if !b.messages.contains_key(name) => lines.push("enum removed".into()),
            (None, Some(_)) if !a.messages.contains_key(name) => lines.push("enum added".into()),
            _ => {}
        }
        if !lines.is_empty() {
            report.entries.push((name.clone(), lines));
        }
    }
    Ok(report)
}

fn describe(f: &FieldDescriptorProto) -> String {
    let ty = match f.r#type() {
        Type::Message | Type::Group | Type::Enum => {
            f.type_name().trim_start_matches('.').to_owned()
        }
        t => t.as_str_name().trim_start_matches("TYPE_").to_lowercase(),
    };
    let label = match f.label() {
        Label::Optional => "optional",
        Label::Required => "required",
        Label::Repeated => "repeated",
    };
    match &f.default_value {
        Some(d) => format!("`{} = {}`: {label} {ty}, default {d}", f.name(), f.number()),
        None => format!("`{} = {}`: {label} {ty}", f.name(), f.number()),
    }
}

fn diff_message(a: &DescriptorProto, b: &DescriptorProto, out: &mut Vec<String>) {
    let by_number = |m: &DescriptorProto| -> BTreeMap<i32, FieldDescriptorProto> {
        m.field.iter().map(|f| (f.number(), f.clone())).collect()
    };
    let (old, new) = (by_number(a), by_number(b));

    for (n, fa) in &old {
        match new.get(n) {
            None => {
                // A field with the same name under another number was renumbered.
                match new
                    .values()
                    .find(|fb| fb.name() == fa.name() && !old.contains_key(&fb.number()))
                {
                    Some(fb) => out.push(format!(
                        "field `{}` renumbered {} -> {}",
                        fa.name(),
                        fa.number(),
                        fb.number()
                    )),
                    None => out.push(format!("removed field {}", describe(fa))),
                }
            }
            Some(fb) if describe(fa) != describe(fb) => out.push(format!(
                "changed field {} -> {}",
                describe(fa),
                describe(fb)
            )),
            Some(_) => {}
        }
    }
    for (n, fb) in &new {
        if old.contains_key(n) {
            continue;
        }
        let renumbered = old
            .values()
            .any(|fa| fa.name() == fb.name() && !new.contains_key(&fa.number()));
        if !renumbered {
            out.push(format!("added field {}", describe(fb)));
        }
    }
}

fn diff_enum(a: &EnumDescriptorProto, b: &EnumDescriptorProto, out: &mut Vec<String>) {
    let by_name = |e: &EnumDescriptorProto| -> BTreeMap<String, i32> {
        e.value
            .iter()
            .map(|v| (v.name().to_owned(), v.number()))
            .collect()
    };
    let (old, new) = (by_name(a), by_name(b));
    for (name, na) in &old {
        match new.get(name) {
            None => out.push(format!("removed value `{name} = {na}`")),
            Some(nb) if nb != na => out.push(format!("value `{name}` renumbered {na} -> {nb}")),
            Some(_) => {}
        }
    }
    for (name, nb) in &new {
        if !old.contains_key(name) {
            out.push(format!("added value `{name} = {nb}`"));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use protox::file::{File, FileResolver};
    use protox::{Compiler, Error};

    use super::*;

    struct Texts(HashMap<String, String>);

    impl FileResolver for Texts {
        fn open_file(&self, name: &str) -> Result<File, Error> {
            match self.0.get(name) {
                Some(src) => File::from_source(name, src),
                None => Err(Error::file_not_found(name)),
            }
        }
    }

    fn compile(src: &str) -> FileDescriptorSet {
        compile_files(&[("a.proto", src)])
    }

    fn compile_files(files: &[(&str, &str)]) -> FileDescriptorSet {
        let texts = files
            .iter()
            .map(|(n, s)| (n.to_string(), s.to_string()))
            .collect();
        let mut c = Compiler::with_file_resolver(Texts(texts));
        c.include_imports(true);
        c.open_file(Path::new(files[0].0)).unwrap();
        c.file_descriptor_set()
    }

    const BASE: &str = r#"syntax = "proto2";
        message Watched { optional uint32 a = 1; optional int32 b = 2 [default = -1]; }
        enum E { K_A = 0; K_B = 1; }"#;

    fn run(old: &str, new: &str, watched: &[&str]) -> Report {
        let w: Vec<String> = watched.iter().map(|s| s.to_string()).collect();
        diff(&compile(old), &compile(new), &w).unwrap()
    }

    fn lines(r: &Report, name: &str) -> Vec<String> {
        r.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, l)| l.clone())
            .unwrap_or_default()
    }

    #[test]
    fn identical_sets_report_nothing() {
        assert!(run(BASE, BASE, &["Watched", "E"]).is_empty());
    }

    #[test]
    fn field_added() {
        let new = BASE.replace("}\n        enum", "optional bool c = 3; }\n        enum");
        let r = run(BASE, &new, &["Watched"]);
        assert_eq!(lines(&r, "Watched"), ["added field `c = 3`: optional bool"]);
    }

    #[test]
    fn field_removed() {
        let new = BASE.replace(" optional int32 b = 2 [default = -1];", "");
        let r = run(BASE, &new, &["Watched"]);
        assert_eq!(
            lines(&r, "Watched"),
            ["removed field `b = 2`: optional int32, default -1"]
        );
    }

    #[test]
    fn field_retyped() {
        let new = BASE.replace("optional uint32 a", "optional uint64 a");
        let r = run(BASE, &new, &["Watched"]);
        assert_eq!(
            lines(&r, "Watched"),
            ["changed field `a = 1`: optional uint32 -> `a = 1`: optional uint64"]
        );
    }

    #[test]
    fn field_renumbered() {
        let new = BASE.replace("uint32 a = 1", "uint32 a = 5");
        let r = run(BASE, &new, &["Watched"]);
        assert_eq!(lines(&r, "Watched"), ["field `a` renumbered 1 -> 5"]);
    }

    #[test]
    fn change_in_reachable_message_is_found() {
        let old = r#"syntax = "proto2";
            message Watched { optional Inner x = 1; }
            message Inner { optional uint32 a = 1; }
            message Unrelated { optional uint32 a = 1; }"#;
        let new = old
            .replace(
                "Inner { optional uint32 a = 1; }",
                "Inner { optional uint32 a = 2; }",
            )
            .replace(
                "Unrelated { optional uint32 a = 1; }",
                "Unrelated { optional uint32 a = 9; }",
            );
        let r = run(old, &new, &["Watched"]);
        assert_eq!(r.entries.len(), 1, "{r:?}");
        assert_eq!(lines(&r, "Inner"), ["field `a` renumbered 1 -> 2"]);
    }

    #[test]
    fn nested_and_imported_messages() {
        let old = [
            (
                "a.proto",
                r#"syntax = "proto2"; import "b.proto";
                message W { message N { optional uint32 v = 1; } optional N n = 1;
                            optional B b = 2; }"#,
            ),
            (
                "b.proto",
                r#"syntax = "proto2"; message B { optional uint32 v = 1; }"#,
            ),
        ];
        let mut new = old;
        new[1].1 =
            r#"syntax = "proto2"; message B { optional uint32 v = 1; optional bool w = 2; }"#;
        let w = vec!["W".to_string()];
        let r = diff(&compile_files(&old), &compile_files(&new), &w).unwrap();
        assert_eq!(lines(&r, "B"), ["added field `w = 2`: optional bool"]);
    }

    #[test]
    fn enum_values() {
        let new = BASE.replace("K_B = 1;", "K_B = 2; K_C = 3;");
        let r = run(BASE, &new, &["E"]);
        assert_eq!(
            lines(&r, "E"),
            ["value `K_B` renumbered 1 -> 2", "added value `K_C = 3`"]
        );
        let new = BASE.replace(" K_B = 1;", "");
        let r = run(BASE, &new, &["E"]);
        assert_eq!(lines(&r, "E"), ["removed value `K_B = 1`"]);
    }

    #[test]
    fn unknown_watched_name_is_an_error() {
        let w = vec!["Nope".to_string()];
        assert!(diff(&compile(BASE), &compile(BASE), &w).is_err());
    }

    #[test]
    fn markdown_lists_each_type() {
        let new = BASE.replace("uint32 a = 1", "uint32 a = 5");
        let md = run(BASE, &new, &["Watched"]).to_markdown();
        assert!(md.contains("### `Watched`"), "{md}");
        assert!(md.contains("- field `a` renumbered 1 -> 5"), "{md}");
    }
}
