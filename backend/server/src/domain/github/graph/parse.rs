//! Tree-sitter supplies the Rust AST and original byte positions. No source-code tokenization.
use std::ops::ControlFlow;
use tree_sitter::{Node, ParseOptions, Parser};

use super::{Coverage, Declaration, Facts, MAX_FILE_BYTES, Parsed, Role, Token, Usage, Visibility};

const MAX_NODES: usize = 65_536;
const MAX_DEPTH: usize = 128;
const MAX_OCCURRENCES: usize = 4096;

pub(super) fn extract(parser: &mut Parser, text: &str) -> Parsed {
    if text.len() > MAX_FILE_BYTES {
        return Parsed::empty(Coverage::Limit);
    }
    let mut polls = 0;
    let mut budget = |_: &tree_sitter::ParseState| {
        polls += 1;
        if polls > MAX_NODES {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut budget);
    let tree = parser.parse_with_options(
        &mut |offset, _| &text.as_bytes()[offset..],
        None,
        Some(options),
    );
    let Some(tree) = tree else {
        parser.reset();
        return Parsed::empty(Coverage::Limit);
    };
    let root = tree.root_node();
    if root.descendant_count() > MAX_NODES {
        return Parsed::empty(Coverage::Limit);
    }
    if root.has_error() {
        return Parsed::empty(Coverage::SyntaxError);
    }
    let mut facts = Facts::default();
    if visit(root, text, true, 0, &mut facts).is_err() {
        return Parsed::empty(Coverage::Limit);
    }
    Parsed {
        coverage: Coverage::Parsed,
        facts,
    }
}

fn attribute_blocks(node: Node<'_>, text: &str) -> bool {
    let Some((attribute, name)) = node
        .named_child(0)
        .and_then(|attribute| Some((attribute, attribute.named_child(0)?)))
    else {
        return true;
    };
    let name = &text[name.byte_range()];
    if matches!(
        name,
        "allow"
            | "warn"
            | "deny"
            | "forbid"
            | "doc"
            | "must_use"
            | "inline"
            | "cold"
            | "deprecated"
    ) {
        return false;
    }
    if node.kind() != "inner_attribute_item"
        || node
            .parent()
            .is_none_or(|parent| parent.kind() != "source_file")
    {
        return true;
    }
    match name {
        "no_std" => attribute.child_count() != 1,
        "cfg_attr" => !doc_cfg_feature(attribute, text),
        _ => true,
    }
}

// Match the token structure of the observed documentation-only feature.
// An extra effect, nested condition, or path override must still block.
fn doc_cfg_feature(attribute: Node<'_>, text: &str) -> bool {
    let Some(arguments) = attribute.child_by_field_name("arguments") else {
        return false;
    };
    let mut cursor = arguments.walk();
    let mut tokens = arguments.children(&mut cursor);
    if !["(", "docsrs", ",", "feature"].into_iter().all(|expected| {
        tokens
            .next()
            .is_some_and(|token| &text[token.byte_range()] == expected)
    }) {
        return false;
    }
    let Some(feature) = tokens.next().filter(|token| token.kind() == "token_tree") else {
        return false;
    };
    if tokens
        .next()
        .is_none_or(|token| &text[token.byte_range()] != ")")
        || tokens.next().is_some()
    {
        return false;
    }
    let mut cursor = feature.walk();
    feature
        .children(&mut cursor)
        .map(|token| &text[token.byte_range()])
        .eq(["(", "doc_cfg", ")"])
}

fn declaration(node: Node<'_>, text: &str, usable: bool) -> Declaration {
    let mut cursor = node.walk();
    let visibility = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == "visibility_modifier")
        .map_or(Visibility::Module, |modifier| {
            // Whitespace or a visibility form outside this subset may hide an edge, never widen it.
            match &text[modifier.byte_range()] {
                "pub" | "pub(crate)" => Visibility::Crate,
                "pub(super)" => Visibility::Parent,
                _ => Visibility::Module,
            }
        });
    Declaration { usable, visibility }
}

fn visit(
    node: Node<'_>,
    text: &str,
    trusted: bool,
    depth: usize,
    facts: &mut Facts,
) -> Result<(), ()> {
    if depth > MAX_DEPTH {
        return Err(());
    }
    let top_level = node.parent().is_some_and(|n| n.kind() == "source_file");
    match node.kind() {
        "function_item" if top_level => {
            if let Some(token) = node
                .child_by_field_name("name")
                .and_then(|n| token(n, text))
            {
                facts
                    .functions
                    .push((token, declaration(node, text, trusted)));
            }
        }
        "mod_item" if top_level => {
            if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|n| token(n, text))
            {
                facts
                    .modules
                    .entry(name.name)
                    .or_default()
                    .push(declaration(
                        node,
                        text,
                        trusted && node.child_by_field_name("body").is_none(),
                    ));
            }
        }
        "call_expression" => {
            if let Some(expr) = node.child_by_field_name("function") {
                capture(expr, Role::Call, text, trusted, facts);
            }
        }
        "use_declaration" => {
            if let Some(mut path) = node.child_by_field_name("argument") {
                if path.kind() == "use_as_clause"
                    && let Some(original) = path.child_by_field_name("path")
                {
                    path = original;
                }
                capture(path, Role::Import, text, trusted, facts);
            }
        }
        _ => {}
    }
    if facts.uses.len() + facts.functions.len() + facts.modules.len() > MAX_OCCURRENCES {
        return Err(());
    }
    let mut cursor = node.walk();
    let scope_trusted = trusted
        && !node
            .named_children(&mut cursor)
            .any(|n| n.kind() == "inner_attribute_item" && attribute_blocks(n, text));
    let mut next_trusted = scope_trusted;
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "attribute_item" => next_trusted &= !attribute_blocks(child, text),
            "inner_attribute_item" | "line_comment" | "block_comment" => {}
            _ => {
                visit(child, text, next_trusted, depth + 1, facts)?;
                next_trusted = scope_trusted;
            }
        }
    }
    Ok(())
}

fn token(node: Node<'_>, text: &str) -> Option<Token> {
    if !matches!(node.kind(), "identifier" | "field_identifier") {
        return None;
    }
    let raw = &text[node.byte_range()];
    let name = raw.strip_prefix("r#").unwrap_or(raw);
    Some(Token {
        name: name.to_owned(),
        start: node.start_byte(),
        end: node.end_byte(),
        line: node.start_position().row + 1,
    })
}

fn capture(expr: Node<'_>, role: Role, text: &str, trusted: bool, facts: &mut Facts) {
    let name = match expr.kind() {
        "scoped_identifier" => expr.child_by_field_name("name"),
        "field_expression" => expr.child_by_field_name("field"),
        "identifier" => Some(expr),
        _ => None,
    };
    if let Some(name) = name.and_then(|n| token(n, text)) {
        facts.uses.push(Usage {
            qualified: trusted.then(|| absolute_path(expr, text)).flatten(),
            token: name,
            role,
        });
    }
}

fn absolute_path(mut path: Node<'_>, text: &str) -> Option<String> {
    let mut segments = Vec::new();
    while path.kind() == "scoped_identifier" {
        if segments.len() >= 32 {
            return None;
        }
        segments.push(token(path.child_by_field_name("name")?, text)?.name);
        path = path.child_by_field_name("path")?;
    }
    if path.kind() != "crate" {
        return None;
    }
    segments.push("crate".to_owned());
    segments.reverse();
    Some(segments.join("::"))
}
