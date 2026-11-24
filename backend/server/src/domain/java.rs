use std::ops::ControlFlow;

use tree_sitter::{ParseOptions, Parser, Tree};

const MAX_AST_STEPS: usize = 65_536;

pub(crate) fn parse_source(text: &str) -> Option<Tree> {
    // Java translates Unicode escapes before lexing; keep this exact parser path conservative.
    if text.contains(r"\u") {
        return None;
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .ok()?;
    let mut steps = 0;
    let mut budget = |_: &tree_sitter::ParseState| {
        steps += 1;
        if steps > MAX_AST_STEPS {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let tree = parser.parse_with_options(
        &mut |offset, _| &text.as_bytes()[offset..],
        None,
        Some(ParseOptions::new().progress_callback(&mut budget)),
    )?;
    let root = tree.root_node();
    (!root.has_error() && root.descendant_count() <= MAX_AST_STEPS).then_some(tree)
}
