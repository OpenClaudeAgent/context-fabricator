use super::*;

fn input<'a>(path: &'a str, text: &'a str) -> Source<'a> {
    Source {
        path,
        blob: blob_id(text),
        text,
    }
}

#[test]
fn crate_navigation_attributes_preserve_exact_unconditional_edges()
-> Result<(), Box<dyn std::error::Error>> {
    let root = concat!(
        "#![no_std]\n#![cfg_attr(docsrs, feature(doc_cfg))]\n",
        "mod child;\n",
        "#[cfg(feature = \"left\")] mod competing;\n",
        "#[cfg(not(feature = \"left\"))] mod competing;\n",
        "use crate::child::selected;\n",
        "fn caller() { crate::child::selected(); crate::competing::conditional(); }\n",
    );
    let child = "pub fn selected() {}";
    let graph = project(
        &[
            input("src/lib.rs", root),
            input("src/child.rs", child),
            input("src/competing.rs", "pub fn conditional() {}"),
        ],
        "src/lib.rs",
    )?;
    assert!(graph.files.iter().any(|file| file.path == "src/lib.rs"
        && file.coverage == Coverage::Parsed
        && file.reachable));
    assert!(graph.files.iter().any(|file| file.path == "src/child.rs"
        && file.coverage == Coverage::Parsed
        && file.reachable));
    let definition = graph
        .definitions
        .iter()
        .find(|d| d.qualified_name == "crate::child::selected")
        .ok_or("unconditional child definition")?;
    assert_eq!(
        &child[definition.site.start_byte..definition.site.end_byte],
        "selected"
    );
    for role in [Role::Import, Role::Call] {
        let observed = graph
            .references
            .iter()
            .find(|r| r.name == "selected" && r.role == role)
            .ok_or("qualified child reference")?;
        assert_eq!(
            &root[observed.site.start_byte..observed.site.end_byte],
            "selected"
        );
        assert_eq!(observed.target.as_ref(), Some(&definition.site));
        assert_eq!(observed.resolution, Resolution::Resolved);
    }
    let conditional = graph
        .references
        .iter()
        .find(|r| r.name == "conditional")
        .ok_or("competing module call")?;
    assert_ne!(conditional.resolution, Resolution::Resolved);
    assert!(conditional.target.is_none());
    Ok(())
}

#[test]
fn crate_attributes_with_other_effects_cannot_enable_navigation() -> Result<(), Error> {
    for attribute in [
        "#![cfg_attr(docsrs, feature(doc_cfg), path = \"alternate.rs\")]",
        "#![cfg_attr(docsrs, cfg(feature = \"other\"))]",
        "#![cfg_attr(docsrs, feature(doc_cfg, other))]",
        "#![cfg_attr(docsrs, feature(other))]",
        "#![cfg_attr(other, feature(doc_cfg))]",
        "#![cfg(feature = \"other\")]",
        "#![no_std(other)]",
        "#![unknown]",
    ] {
        let root = format!("{attribute}\nmod child;\nfn caller() {{ crate::child::selected(); }}");
        let graph = project(
            &[
                input("src/lib.rs", &root),
                input("src/child.rs", "pub fn selected() {}"),
            ],
            "src/lib.rs",
        )?;
        assert!(
            graph
                .files
                .iter()
                .any(|file| file.path == "src/lib.rs" && file.coverage == Coverage::Parsed)
        );
        assert!(
            !graph
                .files
                .iter()
                .any(|file| file.path == "src/child.rs" && file.reachable)
        );
        assert_eq!(graph.references.len(), 1, "{attribute}");
        assert_eq!(
            graph.references[0].resolution,
            Resolution::Unresolved,
            "{attribute}"
        );
        assert!(graph.references[0].target.is_none());
    }
    Ok(())
}

#[test]
fn qualified_call_and_import_reopen_their_original_identifier()
-> Result<(), Box<dyn std::error::Error>> {
    let main = "mod target;\r\nuse crate::target::run as alias;\r\nfn use_it() { /*é*/ crate::target::run(); crate::target::run(); alias(); }";
    let target = "pub fn run() {}";
    let graph = project(
        &[input("src/lib.rs", main), input("src/target.rs", target)],
        "src/lib.rs",
    )?;
    assert_eq!(graph.definitions.len(), 2);
    let resolved: Vec<_> = graph
        .references
        .iter()
        .filter(|r| r.resolution == Resolution::Resolved)
        .collect();
    assert_eq!(resolved.len(), 3);
    assert_eq!(
        resolved.iter().filter(|r| r.role == Role::Import).count(),
        1
    );
    for r in resolved {
        assert_eq!(&main[r.site.start_byte..r.site.end_byte], "run");
        let definition = r.target.as_ref().ok_or("missing resolved target")?;
        assert_eq!(definition.path, "src/target.rs");
        assert_eq!(&target[definition.start_byte..definition.end_byte], "run");
    }
    assert!(
        graph
            .references
            .iter()
            .any(|r| r.name == "alias" && r.resolution == Resolution::Unresolved)
    );
    Ok(())
}

#[test]
fn duplicate_modules_and_conditional_declarations_have_no_guessed_target() -> Result<(), Error> {
    let main = "mod dupe; mod guarded; #[path=\"else.rs\"] mod moved; fn f(){crate::dupe::go();crate::guarded::go();crate::moved::go();}";
    let files = [
        input("lib.rs", main),
        input("dupe.rs", "pub fn go() {}"),
        input("dupe/mod.rs", "pub fn go() {}"),
        input("guarded.rs", "#[cfg(feature=\"x\")] pub fn go() {}"),
        input("moved.rs", "pub fn go() {}"),
    ];
    let graph = project(&files, "lib.rs")?;
    let roles: Vec<_> = graph.references.iter().map(|r| r.resolution).collect();
    assert_eq!(
        roles,
        [
            Resolution::Ambiguous,
            Resolution::Unresolved,
            Resolution::Unresolved
        ]
    );
    assert!(graph.references.iter().all(|r| r.target.is_none()));
    Ok(())
}

#[test]
fn stale_paths_bad_syntax_and_local_shadowing_do_not_bind() -> Result<(), Error> {
    let main = "mod target; mod broken; fn f(){let run = || {}; run(); crate::target::run(); crate::broken::go();}";
    let files = [
        input("lib.rs", main),
        input("new.rs", "pub fn run() {}"),
        input("broken.rs", "fn go( {"),
        input("other.py", "def run(): pass"),
    ];
    let result = project(&files, "lib.rs")?;
    assert!(
        result
            .references
            .iter()
            .all(|r| r.resolution == Resolution::Unresolved)
    );
    assert!(
        result
            .files
            .iter()
            .any(|f| f.coverage == Coverage::SyntaxError)
    );
    assert!(
        result
            .files
            .iter()
            .any(|f| f.coverage == Coverage::UnsupportedLanguage)
    );
    assert_eq!(result.definitions.len(), 1);
    Ok(())
}

#[test]
fn manifest_authenticity_and_parser_limits_are_explicit() -> Result<(), Error> {
    let mut forged = input("lib.rs", "fn x() {}");
    forged.text = "fn y() {}";
    assert_eq!(
        project(&[forged], "lib.rs").err(),
        Some(Error::InvalidSource)
    );
    let duplicate = [input("lib.rs", ""), input("lib.rs", "")];
    assert_eq!(
        project(&duplicate, "lib.rs").err(),
        Some(Error::InvalidSource)
    );
    let too_big = " ".repeat(MAX_FILE_BYTES + 1);
    let result = project(&[input("lib.rs", &too_big)], "lib.rs")?;
    assert_eq!(result.files[0].coverage, Coverage::Limit);
    Ok(())
}

#[test]
fn two_crate_roots_keep_distinct_bindings_for_the_same_symbol() -> Result<(), Error> {
    let code = "mod target; fn call(){crate::target::go();}";
    let files = [
        input("a/lib.rs", code),
        input("b/lib.rs", code),
        input("a/target.rs", "pub fn go() {}"),
        input("b/target.rs", "pub fn go() {}"),
    ];
    for root in ["a", "b"] {
        let graph = project(&files, &format!("{root}/lib.rs"))?;
        let sites: Vec<_> = graph
            .references
            .iter()
            .filter_map(|r| r.target.as_ref())
            .collect();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].path, format!("{root}/target.rs"));
    }
    Ok(())
}

#[test]
fn private_ancestors_constrain_public_functions_and_cfg_calls_are_unresolved() -> Result<(), Error>
{
    let files = [
        input(
            "lib.rs",
            "mod outer; fn f(){crate::outer::hidden::go();crate::outer::other(); #[cfg(test)] crate::outer::ok();}",
        ),
        input(
            "outer.rs",
            "mod hidden; fn other(){} pub fn ok(){} fn local(){crate::outer::hidden::go();}",
        ),
        input("outer/hidden.rs", "pub fn go(){}"),
    ];
    let result = project(&files, "lib.rs")?;
    let calls: Vec<_> = result.references.iter().map(|r| r.resolution).collect();
    assert_eq!(
        calls,
        [
            Resolution::Unresolved,
            Resolution::Unresolved,
            Resolution::Unresolved,
            Resolution::Resolved
        ]
    );
    Ok(())
}

#[test]
#[ignore = "requires the M10 pinned repository objects"]
fn m10_crate_qualified_call_reopens_in_both_pinned_files() -> Result<(), Box<dyn std::error::Error>>
{
    use std::process::Command;
    const TIP: &str = "a3b844b3b59213156866cf913ba7aa228d7a30a8";
    let git = |args: &[&str]| -> Result<String, Box<dyn std::error::Error>> {
        let output = Command::new("git").args(args).output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        Ok(String::from_utf8(output.stdout)?)
    };
    let mut pinned = Vec::new();
    for line in git(&["ls-tree", "-r", "--full-tree", TIP, "backend/server/src"])?.lines() {
        let (header, path) = line.split_once('\t').ok_or("git tree entry")?;
        if rust_path(path) {
            let blob = header.split_whitespace().last().ok_or("immutable blob")?;
            pinned.push((
                path.to_owned(),
                blob.to_owned(),
                git(&["cat-file", "blob", blob])?,
            ));
        }
    }
    let sources = pinned
        .iter()
        .map(|(path, blob, text)| Source {
            path,
            blob: blob.clone(),
            text,
        })
        .collect::<Vec<_>>();
    let result = project(&sources, "backend/server/src/lib.rs")?;
    let reference = result
        .references
        .iter()
        .find(|r| {
            r.name == "valid_branch"
                && r.site.path == "backend/server/src/application/mod.rs"
                && r.site.start_line == 154
        })
        .ok_or("pinned call")?;
    let target = reference.target.as_ref().ok_or("pinned definition")?;
    assert_eq!(target.path, "backend/server/src/domain/github.rs");
    assert_eq!(target.start_line, 227);
    for site in [&reference.site, target] {
        let source = sources
            .iter()
            .find(|f| f.path == site.path && f.blob == site.blob)
            .ok_or("source locator")?;
        assert_eq!(&source.text[site.start_byte..site.end_byte], "valid_branch");
    }
    Ok(())
}
