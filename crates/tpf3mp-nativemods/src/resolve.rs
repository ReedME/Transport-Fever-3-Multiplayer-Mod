//! Which packages an install needs, in the order they go in.
//!
//! A wanted package resolves to its newest version pinned to the running
//! build; a dependency to the installed version when that one satisfies it
//! and runs on the build, else to its newest version that does. There is
//! no backtracking: when that choice does not hold together, the install is
//! refused with the reason, never settled by guessing. Refused:
//!
//! - a package the index does not list, or not for this build;
//! - one that needs plugins ([`crate::index::Plugin`], not built) or a
//!   feature this hook does not have ([`crate::features`]);
//! - a dependency no listed version satisfies, or a cycle of them;
//! - two packages of which one conflicts with the other, installed ones
//!   included.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::{
    features::{self, FeatureSpec},
    index::{Index, Package},
};

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ResolveError {
    #[error("the native mod {0} is not in the index")]
    Unknown(String),
    #[error("no version of {0} runs on this build of the game")]
    NotForBuild(String),
    #[error(
        "{0} needs a native plugin this TPF3-MP cannot load: none for this platform, or one \
         for another plugin interface"
    )]
    NeedsPlugins(String),
    #[error("{package} needs the hook feature {feature}, which this TPF3-MP does not have")]
    UnknownFeature { package: String, feature: String },
    #[error("{needed_by} needs {id} {requirement}, which no version on this build satisfies")]
    NoMatch {
        id: String,
        requirement: String,
        needed_by: String,
    },
    #[error("the native mods depend on each other in a circle: {}", .0.join(" → "))]
    Cycle(Vec<String>),
    #[error("{0} conflicts with {1}")]
    Conflict(String, String),
}

/// What packages are resolved against.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub index: &'a Index,
    /// The running game's executable, by SHA-256.
    pub build: &'a str,
    /// The features this hook has ([`features::BUILT_IN`]).
    pub registry: &'a [FeatureSpec],
}

/// The packages selected for a request and the complete dependency closure.
/// `install` is dependency-first and omits already-installed packages that
/// already match. `closure` includes those cached choices for preflight.
#[derive(Debug, Clone)]
pub struct Resolution<'a> {
    pub install: Vec<&'a Package>,
    pub closure: Vec<&'a Package>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Visit {
    Visiting,
    Done,
}

struct Walk<'a> {
    context: Context<'a>,
    installed: BTreeMap<&'a str, &'a Package>,
    state: BTreeMap<String, Visit>,
    chosen: BTreeMap<String, &'a Package>,
    order: Vec<&'a Package>,
    path: Vec<String>,
}

/// The packages to install for `wanted`, dependencies first, given the
/// packages `installed` now (which stay unless chosen in another version).
pub fn resolve<'a>(
    context: Context<'a>,
    wanted: &[&str],
    installed: &[&'a Package],
) -> Result<Vec<&'a Package>, ResolveError> {
    Ok(resolve_plan(context, wanted, installed)?.install)
}

/// Resolves `wanted` without changing the store, returning both the
/// dependency-first install list and every selected package in its closure.
pub fn resolve_plan<'a>(
    context: Context<'a>,
    wanted: &[&str],
    installed: &[&'a Package],
) -> Result<Resolution<'a>, ResolveError> {
    let wanted: Vec<(&str, Option<semver::VersionReq>)> =
        wanted.iter().map(|id| (*id, None)).collect();
    resolve_pinned(context, &wanted, installed)
}

/// The packages to install for `id` in exactly `version` (a room's, which
/// a member must run alike), dependencies first, as [`resolve`].
pub fn resolve_exact<'a>(
    context: Context<'a>,
    id: &str,
    version: &str,
    installed: &[&'a Package],
) -> Result<Vec<&'a Package>, ResolveError> {
    Ok(resolve_exact_plan(context, id, version, installed)?.install)
}

/// Resolves `id` in exactly `version`, without changing the store, returning
/// the full dependency closure as well as what must be installed.
pub fn resolve_exact_plan<'a>(
    context: Context<'a>,
    id: &str,
    version: &str,
    installed: &[&'a Package],
) -> Result<Resolution<'a>, ResolveError> {
    let exact = semver::VersionReq::parse(&format!("={version}"))
        .map_err(|_| ResolveError::Unknown(format!("{id} {version}")))?;
    resolve_pinned(context, &[(id, Some(exact))], installed)
}

fn resolve_pinned<'a>(
    context: Context<'a>,
    wanted: &[(&str, Option<semver::VersionReq>)],
    installed: &[&'a Package],
) -> Result<Resolution<'a>, ResolveError> {
    let mut walk = Walk {
        context,
        installed: installed.iter().map(|p| (p.id.as_str(), *p)).collect(),
        state: BTreeMap::new(),
        chosen: BTreeMap::new(),
        order: Vec::new(),
        path: Vec::new(),
    };
    for (id, requirement) in wanted {
        walk.visit(id, requirement.as_ref(), true)?;
    }
    // Everything that will be installed afterwards: the chosen packages,
    // and the installed ones nothing replaces.
    let mut after: BTreeMap<&str, &Package> = walk.installed.clone();
    for (id, package) in &walk.chosen {
        after.insert(id.as_str(), package);
    }
    for package in after.values() {
        for other in &package.conflicts {
            if other != &package.id && after.contains_key(other.as_str()) {
                return Err(ResolveError::Conflict(package.id.clone(), other.clone()));
            }
        }
    }
    Ok(Resolution {
        install: walk.order,
        closure: walk.chosen.into_values().collect(),
    })
}

impl<'a> Walk<'a> {
    fn visit(
        &mut self,
        id: &str,
        requirement: Option<&semver::VersionReq>,
        wanted: bool,
    ) -> Result<(), ResolveError> {
        let needed_by = self.path.last().cloned().unwrap_or_default();
        let no_match = |requirement: &semver::VersionReq| ResolveError::NoMatch {
            id: id.to_owned(),
            requirement: requirement.to_string(),
            needed_by: needed_by.clone(),
        };
        match self.state.get(id) {
            Some(Visit::Visiting) => {
                let start = self.path.iter().position(|p| p == id).unwrap_or(0);
                let mut cycle = self.path[start..].to_vec();
                cycle.push(id.to_owned());
                return Err(ResolveError::Cycle(cycle));
            }
            Some(Visit::Done) => {
                let chosen = self.chosen[id];
                return match requirement {
                    Some(requirement) if !requirement.matches(&chosen.semver()) => {
                        Err(no_match(requirement))
                    }
                    _ => Ok(()),
                };
            }
            None => {}
        }
        let index = self.context.index;
        let build = self.context.build;
        let accepts = |p: &Package| requirement.is_none_or(|r| r.matches(&p.semver()));
        // A dependency keeps the installed version when it will do.
        let kept = (!wanted)
            .then(|| self.installed.get(id).copied())
            .flatten()
            .filter(|p| p.runs_on(build) && accepts(p));
        let package = match kept {
            Some(installed) => index
                .package(id, &installed.version)
                .filter(|package| package.runs_on(build) && accepts(package))
                // When the signed index still offers this exact version,
                // use its current manifest even if the installed version
                // number already satisfies the dependency.
                .unwrap_or(installed),
            None => {
                let versions = index.versions(id);
                if versions.is_empty() {
                    return Err(ResolveError::Unknown(id.to_owned()));
                }
                let on_build: Vec<&Package> =
                    versions.into_iter().filter(|p| p.runs_on(build)).collect();
                if on_build.is_empty() {
                    return Err(ResolveError::NotForBuild(id.to_owned()));
                }
                match on_build.into_iter().find(|p| accepts(p)) {
                    Some(package) => package,
                    None => return Err(no_match(requirement.unwrap_or(&semver::VersionReq::STAR))),
                }
            }
        };
        if !package.plugins.is_empty() {
            let here = crate::index::current_platform()
                .map(|platform| package.plugins_for(platform))
                .unwrap_or_default();
            if here.is_empty() || here.iter().any(|p| p.abi != crate::index::PLUGIN_ABI) {
                return Err(ResolveError::NeedsPlugins(id.to_owned()));
            }
        }
        if let Some(feature) = package
            .features
            .iter()
            .find(|f| features::find(self.context.registry, f).is_none())
        {
            return Err(ResolveError::UnknownFeature {
                package: id.to_owned(),
                feature: feature.clone(),
            });
        }
        self.state.insert(id.to_owned(), Visit::Visiting);
        self.path.push(id.to_owned());
        for dependency in &package.depends {
            // Validated with the index.
            let requirement =
                semver::VersionReq::parse(&dependency.version).unwrap_or(semver::VersionReq::STAR);
            self.visit(&dependency.id, Some(&requirement), false)?;
        }
        self.path.pop();
        self.state.insert(id.to_owned(), Visit::Done);
        self.chosen.insert(id.to_owned(), package);
        // An exact room install must revisit an already-installed version
        // when the signed index changed its package manifest without a
        // version bump. `Store::install_one` verifies the on-disk files
        // against this package and replaces a stale same-version copy.
        if self
            .installed
            .get(id)
            .is_none_or(|installed| *installed != package)
        {
            self.order.push(package);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        features::example::BIG_MAPS,
        index::{
            Dependency, Plugin,
            tests::{BUILD, OTHER_BUILD, big_maps, index, package},
        },
    };

    #[test]
    fn a_room_gets_its_exact_version_not_the_newest() {
        let mut newer = big_maps();
        newer.version = "0.4.0".into();
        let index = index(vec![big_maps(), newer]);
        let context = Context {
            index: &index,
            build: BUILD,
            registry: BIG_MAPS,
        };
        let newest = resolve(context, &["bigmap"], &[]).unwrap();
        assert_eq!(newest[0].version, "0.4.0");
        let exact = resolve_exact(context, "bigmap", "0.3.0", &[]).unwrap();
        assert_eq!(exact[0].version, "0.3.0");
        assert!(matches!(
            resolve_exact(context, "bigmap", "9.9.9", &[]),
            Err(ResolveError::NoMatch { .. })
        ));
    }

    fn needs(mut package: Package, id: &str, version: &str) -> Package {
        package.depends.push(Dependency {
            id: id.into(),
            version: version.into(),
        });
        package
    }

    fn ids(order: &[&Package]) -> Vec<String> {
        order
            .iter()
            .map(|p| format!("{} {}", p.id, p.version))
            .collect()
    }

    fn context(index: &Index) -> Context<'_> {
        Context {
            index,
            build: BUILD,
            registry: BIG_MAPS,
        }
    }

    #[test]
    fn the_newest_version_for_the_build_comes_after_its_dependencies() {
        let mut newest = package("a", "2.0.0", &[]);
        newest.builds = vec![OTHER_BUILD.into()];
        let index = index(vec![
            needs(package("a", "1.1.0", &[]), "b", "^1"),
            package("a", "1.0.0", &[]),
            newest,
            package("b", "1.4.0", &[]),
            package("b", "2.0.0", &[]),
        ]);
        let order = resolve(context(&index), &["a"], &[]).unwrap();
        assert_eq!(ids(&order), ["b 1.4.0", "a 1.1.0"]);
    }

    #[test]
    fn a_package_not_for_this_build_is_refused() {
        let mut only_other = package("a", "1.0.0", &[]);
        only_other.builds = vec![OTHER_BUILD.into()];
        let index = index(vec![only_other]);
        assert_eq!(
            resolve(context(&index), &["a"], &[]),
            Err(ResolveError::NotForBuild("a".into()))
        );
        assert_eq!(
            resolve(context(&index), &["nope"], &[]),
            Err(ResolveError::Unknown("nope".into()))
        );
    }

    #[test]
    fn unknown_features_and_plugins_are_refused() {
        let index = index(vec![big_maps()]);
        // This hook has no Big Maps.
        let bare = Context {
            registry: crate::features::BUILT_IN,
            ..context(&index)
        };
        assert!(matches!(
            resolve(bare, &["bigmap"], &[]),
            Err(ResolveError::UnknownFeature { .. })
        ));
        assert_eq!(
            ids(&resolve(context(&index), &["bigmap"], &[]).unwrap()),
            ["bigmap 0.3.0"]
        );
        // A plugin for this platform and interface installs; one for
        // another platform, or another interface, does not.
        let plugin = |platform: &str, abi: u32| Plugin {
            path: format!("p-{platform}.bin"),
            url: "https://test/p.bin".into(),
            size: 1,
            sha256: "0".repeat(64),
            abi,
            platform: platform.into(),
        };
        let with = |plugins: Vec<Plugin>| {
            let mut plugged = package("p", "1.0.0", &[]);
            plugged.plugins = plugins;
            super::super::index::tests::index(vec![plugged])
        };
        let here = crate::index::current_platform();
        let elsewhere = crate::index::PLATFORMS
            .iter()
            .find(|p| Some(**p) != here)
            .unwrap();
        let index = with(vec![plugin(elsewhere, crate::index::PLUGIN_ABI)]);
        assert_eq!(
            resolve(context(&index), &["p"], &[]),
            Err(ResolveError::NeedsPlugins("p".into()))
        );
        let linux = with(vec![plugin("linux-x86_64", crate::index::PLUGIN_ABI)]);
        assert_eq!(
            resolve(context(&linux), &["p"], &[]),
            Err(ResolveError::NeedsPlugins("p".into()))
        );
        if let Some(here) = here {
            let index = with(vec![plugin(here, crate::index::PLUGIN_ABI)]);
            assert_eq!(
                ids(&resolve(context(&index), &["p"], &[]).unwrap()),
                ["p 1.0.0"]
            );
            let index = with(vec![plugin(here, crate::index::PLUGIN_ABI + 1)]);
            assert_eq!(
                resolve(context(&index), &["p"], &[]),
                Err(ResolveError::NeedsPlugins("p".into()))
            );
        }
    }

    #[test]
    fn a_dependency_cycle_is_refused() {
        let index = index(vec![
            needs(package("a", "1.0.0", &[]), "b", "*"),
            needs(package("b", "1.0.0", &[]), "c", "*"),
            needs(package("c", "1.0.0", &[]), "a", "*"),
        ]);
        assert_eq!(
            resolve(context(&index), &["a"], &[]),
            Err(ResolveError::Cycle(vec![
                "a".into(),
                "b".into(),
                "c".into(),
                "a".into()
            ]))
        );
        let selfish = index_of(needs(package("s", "1.0.0", &[]), "s", "*"));
        assert!(matches!(
            resolve(context(&selfish), &["s"], &[]),
            Err(ResolveError::Cycle(_))
        ));
    }

    fn index_of(package: Package) -> Index {
        index(vec![package])
    }

    #[test]
    fn an_unsatisfiable_dependency_is_refused() {
        let index = index(vec![
            needs(package("a", "1.0.0", &[]), "b", "^2"),
            package("b", "1.0.0", &[]),
        ]);
        assert!(matches!(
            resolve(context(&index), &["a"], &[]),
            Err(ResolveError::NoMatch { .. })
        ));
        // Two packages needing different versions of one.
        let index = super::super::index::tests::index(vec![
            needs(package("x", "1.0.0", &[]), "b", "^1"),
            needs(package("y", "1.0.0", &[]), "b", "^2"),
            package("b", "1.0.0", &[]),
            package("b", "2.0.0", &[]),
        ]);
        assert!(matches!(
            resolve(context(&index), &["x", "y"], &[]),
            Err(ResolveError::NoMatch { .. })
        ));
    }

    #[test]
    fn conflicts_are_refused_installed_ones_included() {
        let mut a = package("a", "1.0.0", &[]);
        a.conflicts.push("b".into());
        let b = package("b", "1.0.0", &[]);
        let index = index(vec![a.clone(), b.clone()]);
        assert_eq!(
            resolve(context(&index), &["a", "b"], &[]),
            Err(ResolveError::Conflict("a".into(), "b".into()))
        );
        // b installed, a wanted: and the other way around.
        assert!(matches!(
            resolve(context(&index), &["a"], &[&b]),
            Err(ResolveError::Conflict(..))
        ));
        assert!(matches!(
            resolve(context(&index), &["b"], &[&a]),
            Err(ResolveError::Conflict(..))
        ));
    }

    #[test]
    fn an_installed_dependency_that_will_do_is_kept() {
        let index = index(vec![
            needs(package("a", "1.0.0", &[]), "b", "^1"),
            package("b", "1.0.0", &[]),
            package("b", "1.5.0", &[]),
        ]);
        let installed_b = index.package("b", "1.0.0").unwrap();
        let order = resolve(context(&index), &["a"], &[installed_b]).unwrap();
        assert_eq!(ids(&order), ["a 1.0.0"]);
        // Wanted itself, it goes to the newest.
        let order = resolve(context(&index), &["b"], &[installed_b]).unwrap();
        assert_eq!(ids(&order), ["b 1.5.0"]);
    }

    #[test]
    fn the_preflight_plan_includes_cached_dependencies_and_updated_signed_manifests() {
        let installed_b = package("b", "1.0.0", &[("old.lua", b"old")]);
        let updated_b = package("b", "1.0.0", &[("new.lua", b"new")]);
        let signed_index = index(vec![
            needs(package("a", "1.0.0", &[]), "b", "^1"),
            updated_b,
        ]);
        let plan = resolve_plan(context(&signed_index), &["a"], &[&installed_b]).unwrap();
        assert_eq!(ids(&plan.install), ["b 1.0.0", "a 1.0.0"]);
        assert_eq!(
            plan.closure
                .iter()
                .map(|package| package.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );

        let installed_only_index = index(vec![needs(package("a", "1.0.0", &[]), "b", "^1")]);
        let plan = resolve_plan(context(&installed_only_index), &["a"], &[&installed_b]).unwrap();
        assert_eq!(ids(&plan.install), ["a 1.0.0"]);
        assert_eq!(
            plan.closure
                .iter()
                .map(|package| package.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        let exact = resolve_exact_plan(
            context(&installed_only_index),
            "a",
            "1.0.0",
            &[&installed_b],
        )
        .unwrap();
        assert_eq!(ids(&exact.install), ["a 1.0.0"]);
        assert_eq!(
            exact
                .closure
                .iter()
                .map(|package| package.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }
}
