//! Bind only absolute function references reached through proven external Rust modules.
use std::collections::{BTreeMap, BTreeSet};

use super::{
    Coverage, Definition, Error, Parsed, Projection, Reference, Resolution, Source, Span,
    Visibility,
};

struct Module<'a> {
    path: &'a str,
    qualified: String,
    directory: String,
    boundary: String,
}

struct Target {
    site: Span,
    boundary: String,
}

impl Module<'_> {
    fn boundary(&self, visibility: Visibility) -> String {
        let required = match visibility {
            Visibility::Crate => "crate",
            Visibility::Parent => self
                .qualified
                .rsplit_once("::")
                .map_or("crate", |(parent, _)| parent),
            Visibility::Module => self.qualified.as_str(),
        };
        if required.len() > self.boundary.len() {
            required.to_owned()
        } else {
            self.boundary.clone()
        }
    }
}

pub(super) fn connect<'a>(
    root: &'a str,
    paths: &BTreeMap<&'a str, &Source<'_>>,
    parsed: &BTreeMap<&str, Parsed>,
    result: &mut Projection,
) -> Result<BTreeSet<&'a str>, Error> {
    let mut reachable = BTreeMap::new();
    let mut targets: BTreeMap<String, Vec<Option<Target>>> = BTreeMap::new();
    let mut ambiguous_modules = BTreeSet::new();
    let directory = root
        .rsplit_once('/')
        .map_or(String::new(), |(parent, _)| format!("{parent}/"));
    let mut pending = vec![Module {
        path: root,
        qualified: "crate".to_owned(),
        directory,
        boundary: "crate".to_owned(),
    }];
    while let Some(module) = pending.pop() {
        if reachable
            .insert(module.path, module.qualified.clone())
            .is_some()
        {
            return Err(Error::InvalidSource);
        }
        let source = paths[module.path];
        let file = &parsed[source.blob.as_str()];
        if file.coverage != Coverage::Parsed {
            continue;
        }
        for (token, declaration) in &file.facts.functions {
            let qualified = format!("{}::{}", module.qualified, token.name);
            let site = token.span(source);
            targets
                .entry(qualified.clone())
                .or_default()
                .push(declaration.usable.then(|| Target {
                    site: site.clone(),
                    boundary: module.boundary(declaration.visibility),
                }));
            if declaration.usable {
                result.definitions.push(Definition {
                    name: token.name.clone(),
                    qualified_name: qualified,
                    site,
                });
            }
        }
        for (name, declarations) in &file.facts.modules {
            let qualified = format!("{}::{name}", module.qualified);
            let child = format!("{}{name}", module.directory);
            let flat = paths.get_key_value(format!("{child}.rs").as_str());
            let nested = paths.get_key_value(format!("{child}/mod.rs").as_str());
            if declarations.len() > 1 || (flat.is_some() && nested.is_some()) {
                ambiguous_modules.insert(qualified);
                continue;
            }
            if let [declaration] = declarations.as_slice()
                && declaration.usable
                && let Some((path, _)) = flat.or(nested)
            {
                pending.push(Module {
                    path,
                    qualified,
                    boundary: module.boundary(declaration.visibility),
                    directory: format!("{child}/"),
                });
            }
        }
    }
    for (path, caller) in &reachable {
        let source = paths[path];
        for usage in &parsed[source.blob.as_str()].facts.uses {
            let (resolution, target) = lookup(
                usage.qualified.as_deref(),
                caller,
                &targets,
                &ambiguous_modules,
            );
            result.references.push(Reference {
                name: usage.token.name.clone(),
                site: usage.token.span(source),
                role: usage.role,
                resolution,
                target,
            });
        }
    }
    Ok(reachable.into_keys().collect())
}

fn lookup(
    name: Option<&str>,
    caller: &str,
    targets: &BTreeMap<String, Vec<Option<Target>>>,
    ambiguous: &BTreeSet<String>,
) -> (Resolution, Option<Span>) {
    let Some(name) = name else {
        return (Resolution::Unresolved, None);
    };
    if name
        .match_indices("::")
        .any(|(index, _)| ambiguous.contains(&name[..index]))
    {
        return (Resolution::Ambiguous, None);
    }
    match targets.get(name).map(Vec::as_slice) {
        Some([Some(target)])
            if caller == target.boundary
                || caller
                    .strip_prefix(&target.boundary)
                    .is_some_and(|suffix| suffix.starts_with("::")) =>
        {
            (Resolution::Resolved, Some(target.site.clone()))
        }
        Some(matches) if matches.len() > 1 => (Resolution::Ambiguous, None),
        _ => (Resolution::Unresolved, None),
    }
}
