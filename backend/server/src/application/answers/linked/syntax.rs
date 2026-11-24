//! Positive Rust syntax checks for bounded source fragments.

use std::ops::Range;

use tree_sitter::{Node, Parser, Tree};

use super::super::quantities;

fn parse(text: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(text, None)?;
    (tree.root_node().descendant_count() <= 65_536).then_some(tree)
}

fn nodes(root: Node<'_>) -> Vec<Node<'_>> {
    let mut pending = vec![root];
    let mut result = Vec::new();
    while let Some(node) = pending.pop() {
        result.push(node);
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        pending.extend(children.into_iter().rev());
    }
    result
}

fn line_range(text: &str, offset: usize) -> Range<usize> {
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    start..text[start..end].trim_end_matches('\r').len() + start
}

pub(super) fn declaration_symbol(text: &str) -> Option<(&str, Range<usize>)> {
    let tree = parse(text)?;
    let mut result = None;
    for node in nodes(tree.root_node()) {
        if node.kind() != "const_item" || !unconditional_root_item(node, text) {
            continue;
        }
        let range = line_range(text, node.start_byte());
        let Some((symbol, _)) = quantities::integer_constant(&text[range.clone()]) else {
            continue;
        };
        let name = node.child_by_field_name("name")?;
        if &text[name.byte_range()] != symbol || symbol.is_empty() {
            continue;
        }
        if result.is_some() {
            return None;
        }
        result = Some((symbol, range));
    }
    result
}

fn has_attribute(node: Node<'_>) -> bool {
    node.prev_named_sibling()
        .is_some_and(|previous| previous.kind() == "attribute_item")
}

fn unconditional_root_item(node: Node<'_>, text: &str) -> bool {
    if node.has_error() {
        return false;
    }
    let root_item = |item: Node<'_>| {
        item.parent()
            .is_some_and(|parent| parent.kind() == "source_file")
            && !has_attribute(item)
    };
    if root_item(node) {
        return true;
    }
    // A truncated final block can cause tree-sitter to recover complete
    // earlier items under ERROR. Retry through the item's actual end.
    text.get(..node.end_byte())
        .and_then(parse)
        .is_some_and(|tree| {
            let root = tree.root_node();
            let mut cursor = root.walk();
            !root.has_error()
                && root.named_children(&mut cursor).any(|item| {
                    item.kind() == node.kind()
                        && item.byte_range() == node.byte_range()
                        && root_item(item)
                })
        })
}

// Only conventional src module paths can be tied to an explicit crate import.
pub(super) fn module_path(path: &str) -> Option<(String, String)> {
    let (root, relative) = path
        .rsplit_once("/src/")
        .or_else(|| path.strip_prefix("src/").map(|rest| ("", rest)))?;
    let relative = relative.strip_suffix(".rs")?;
    if relative == "main" || relative.starts_with("bin/") {
        return None;
    }
    let module = if matches!(relative, "lib" | "mod") {
        ""
    } else {
        relative.strip_suffix("/mod").unwrap_or(relative)
    };
    if !module
        .split('/')
        .all(|part| part.is_empty() || part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
    {
        return None;
    }
    Some((root.to_owned(), module.replace('/', "::")))
}

pub(super) fn imports(text: &str, target: &str) -> bool {
    let Some(tree) = parse(text) else {
        return false;
    };
    nodes(tree.root_node()).into_iter().any(|node| {
        node.kind() == "use_declaration"
            && unconditional_root_item(node, text)
            && !nodes(node)
                .iter()
                .any(|child| child.kind() == "visibility_modifier")
            && node
                .child_by_field_name("argument")
                .is_some_and(|argument| import_path(argument, text, "", target))
    })
}

fn import_path(node: Node<'_>, text: &str, prefix: &str, target: &str) -> bool {
    match node.kind() {
        "use_list" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .any(|child| import_path(child, text, prefix, target))
        }
        "scoped_use_list" => {
            let Some(path) = node.child_by_field_name("path") else {
                return false;
            };
            let next = format!("{}{}::", prefix, &text[path.byte_range()]);
            node.child_by_field_name("list")
                .is_some_and(|list| import_path(list, text, &next, target))
        }
        "identifier" | "scoped_identifier" => {
            format!("{}{}", prefix, &text[node.byte_range()]) == target
        }
        // Aliases, glob imports and re-exports are not identity evidence.
        _ => false,
    }
}

pub(super) fn has_declaration(text: &str, symbol: &str) -> bool {
    parse(text).is_some_and(|tree| {
        nodes(tree.root_node()).into_iter().any(|node| {
            matches!(node.kind(), "const_item" | "static_item")
                && node
                    .child_by_field_name("name")
                    .is_some_and(|name| &text[name.byte_range()] == symbol)
        })
    })
}

// Reject a fragment if a positive syntax pattern could bind the same name.
fn has_binding(all: &[Node<'_>], text: &str, symbol: &str) -> bool {
    all.iter().any(|node| {
        let pattern = node.child_by_field_name("pattern");
        pattern.is_some_and(|pattern| {
            nodes(pattern)
                .into_iter()
                .any(|part| part.kind() == "identifier" && &text[part.byte_range()] == symbol)
        })
    })
}

// A complete expression or let initializer is trustworthy even if the enclosing
// function's closing brace falls in the next indexed passage.
fn executable(node: Node<'_>) -> bool {
    let mut parent = node.parent();
    let mut expression = false;
    while let Some(current) = parent {
        match current.kind() {
            "ERROR"
            | "use_declaration"
            | "const_item"
            | "static_item"
            | "attribute_item"
            | "macro_definition"
            | "type_item"
            | "scoped_identifier"
            | "function_signature_item"
            | "source_file" => return false,
            "let_declaration" if !expression => {
                expression = !current.has_error()
                    && current.child_by_field_name("value").is_some_and(|value| {
                        value.start_byte() <= node.start_byte()
                            && node.end_byte() <= value.end_byte()
                    });
                if !expression {
                    return false;
                }
            }
            "expression_statement" if !expression => expression = !current.has_error(),
            "function_item" => return expression && module_function(current),
            _ => {}
        }
        parent = current.parent();
    }
    false
}

fn module_function(node: Node<'_>) -> bool {
    if has_attribute(node) || node.child_by_field_name("body").is_none() {
        return false;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() == "source_file" {
        return true;
    }
    parent.parent().is_some_and(|item| {
        parent.kind() == "declaration_list"
            && item.kind() == "impl_item"
            && !has_attribute(item)
            && item
                .parent()
                .is_some_and(|scope| scope.kind() == "source_file")
    })
}

pub(super) fn usage_window(
    text: &str,
    symbol: &str,
) -> Option<(Range<usize>, Range<usize>, usize)> {
    if let Some(usage) = parse(text).and_then(|tree| find_usage(&tree, text, symbol)) {
        return Some(usage);
    }
    // The indexed chunk may end midway through a later expression. Let the
    // parser close open blocks after a complete line. Only original bytes can
    // qualify as the use site or appear in the returned ranges.
    text.match_indices(symbol).take(4).find_map(|(offset, _)| {
        let end = line_range(text, offset).end;
        let repaired = format!("{}\n}}\n}}\n}}\n}}\n}}\n}}", &text[..end]);
        find_usage(&parse(&repaired)?, text, symbol)
    })
}

fn find_usage(
    tree: &Tree,
    text: &str,
    symbol: &str,
) -> Option<(Range<usize>, Range<usize>, usize)> {
    let all = nodes(tree.root_node());
    if has_binding(&all, text, symbol) {
        return None;
    }
    let mut best = None;
    for node in &all {
        if node.kind() != "identifier"
            || text.get(node.byte_range()) != Some(symbol)
            || !executable(*node)
        {
            continue;
        }
        let usage = line_range(text, node.start_byte());
        let start = text[..usage.start]
            .match_indices('\n')
            .rev()
            .nth(6)
            .map_or(0, |(i, _)| i + 1);
        let end = text[usage.end..]
            .match_indices('\n')
            .nth(6)
            .map_or(text.len(), |(i, _)| usage.end + i + 1);
        let visible = start..end;
        let score = all
            .iter()
            .filter(|candidate| {
                candidate.start_byte() >= visible.start
                    && candidate.start_byte() < visible.end
                    && matches!(
                        candidate.kind(),
                        "if_expression" | "match_expression" | "while_expression" | "function_item"
                    )
            })
            .count();
        if best
            .as_ref()
            .is_none_or(|(_, _, previous)| score > *previous)
        {
            best = Some((visible, usage, score));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_one_real_integer_declaration() {
        assert_eq!(
            declaration_symbol("// const BAD: u64 = 1;\npub const WAIT_MS: u64 = 12_000;\n")
                .map(|x| x.0),
            Some("WAIT_MS")
        );
        assert!(declaration_symbol("const A: u32 = 1;\nconst B: i64 = -2;\n").is_none());
        assert!(declaration_symbol("const A: u32 = 12 + 1;").is_none());
        assert!(declaration_symbol("const A: u32 = 12u32;").is_none());
    }

    #[test]
    fn complete_root_items_survive_a_truncated_later_block() {
        let text = "use crate::policy::{Outcome, WAIT_MS};\npub const PAUSE_MS: i64 = 10;\n\
                    fn run() {\n    if active {\n        match";
        assert!(imports(text, "crate::policy::WAIT_MS"));
        assert_eq!(
            declaration_symbol(text).map(|item| item.0),
            Some("PAUSE_MS")
        );
        let nested = "mod example {\n    const PAUSE_MS: i64 = 10;\n    fn run() {";
        assert!(declaration_symbol(nested).is_none());
    }

    #[test]
    fn skips_imports_comments_strings_and_other_names() -> Result<(), &'static str> {
        let text = "use crate::WAIT_MS;\nfn run() {\n// WAIT_MS\nlet s = r#\"WAIT_MS\"#;\nlet WAIT_MS_EXTRA = 2;\nlet é = WAIT_MS;\n}\n";
        let (_, usage, _) = usage_window(text, "WAIT_MS").ok_or("missing executable reference")?;
        assert_eq!(&text[usage], "let é = WAIT_MS;");
        assert!(
            usage_window(
                "use x::WAIT_MS;\n// WAIT_MS\nfn x() { let s = r#\"WAIT_MS\"#; }",
                "WAIT_MS"
            )
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn accepts_complete_statement_in_truncated_function() -> Result<(), &'static str> {
        let text =
            "struct A;\nimpl Drop for A {\n fn drop(&mut self) {\n  let deadline = WAIT_MS;\n";
        let (visible, usage, _) = usage_window(text, "WAIT_MS").ok_or("missing recovered usage")?;
        assert_eq!(&text[usage], "  let deadline = WAIT_MS;");
        assert!(text[visible].contains("impl Drop"));
        Ok(())
    }
    #[test]
    fn rejects_other_declarations_and_local_bindings() {
        assert!(has_declaration("static WAIT_MS: u64 = 1;", "WAIT_MS"));
        assert!(!has_declaration(
            "// const WAIT_MS: u64 = 1;\nfn x() { let s = \"WAIT_MS\"; }",
            "WAIT_MS"
        ));
        assert!(usage_window("fn x(WAIT_MS: u64) { consume(WAIT_MS); }", "WAIT_MS").is_none());
        assert!(usage_window("fn x() { let WAIT_MS = 1; consume(WAIT_MS); }", "WAIT_MS").is_none());
        assert!(usage_window("fn x() { let other = 1; consume(WAIT_MS); }", "WAIT_MS").is_some());
    }
    #[test]
    fn requires_direct_local_import_and_rejects_qualified_uses() {
        assert_eq!(
            module_path("backend/server/src/domain/sync.rs"),
            Some(("backend/server".into(), "domain::sync".into()))
        );
        assert!(imports(
            "use crate::domain::sync::{Thing, WAIT_MS};",
            "crate::domain::sync::WAIT_MS"
        ));
        assert!(imports(
            "use crate::policy::WAIT_MS;",
            "crate::policy::WAIT_MS"
        ));
        assert!(!imports(
            "use external::policy::WAIT_MS;",
            "crate::policy::WAIT_MS"
        ));
        assert!(!imports(
            "pub use crate::policy::WAIT_MS;",
            "crate::policy::WAIT_MS"
        ));
        assert!(!imports(
            "use crate::policy::WAIT_MS as OTHER;",
            "crate::policy::WAIT_MS"
        ));
        assert!(!imports(
            "#[cfg(test)] use crate::policy::WAIT_MS;",
            "crate::policy::WAIT_MS"
        ));
        assert!(!imports(
            "mod tests { use crate::policy::WAIT_MS; }",
            "crate::policy::WAIT_MS"
        ));
        assert!(usage_window("fn x() { let n = external::WAIT_MS; }", "WAIT_MS").is_none());
        assert!(usage_window("#[cfg(test)] fn x() { let n = WAIT_MS; }", "WAIT_MS").is_none());
        assert!(usage_window("mod test { fn x() { let n = WAIT_MS; } }", "WAIT_MS").is_none());
        assert!(module_path("backend/src/bin/tool.rs").is_none());
        assert!(declaration_symbol("mod inner { const WAIT_MS: u64 = 1; }").is_none());
    }
}
