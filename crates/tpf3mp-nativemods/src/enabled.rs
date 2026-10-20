//! What the launcher hands the game, and what the hook makes of it.
//!
//! The launcher writes the packages the player enabled, for the build it
//! starts, to `native-mods/enabled.json` in its data folder, and names that
//! file in the game's environment ([`ENV`]), beside the game's link (D11).
//! No variable, nothing enabled: a game started any other way runs no
//! native mod. No other switch turns a feature on (Big Maps' own branches
//! read `TPF3MP_BIGMAP_*` variables; as a package it would not).
//!
//! The hook then switches a package's features on only when all of this
//! holds ([`plan`]):
//!
//! - the file names the build the hook is running in (the executable's
//!   SHA-256, the same one its profile matched);
//! - the hook has every feature the package names ([`crate::features`]),
//!   and the matched profile has every target those features patch;
//! - for a package that changes the simulation, the room compares native
//!   mods ([`ROOM_TERMS_CARRIED`]).
//!
//! Packages are switched on in the order the file lists them, the **load
//! order** ([`load_order`]): a package after those it depends on, the rest
//! by id, the same on every player's game. A package whose features clash
//! with those of one before it ([`features::clash`]: both patch a target
//! one takes over, or would combine it differently) does not run, as if it
//! lacked a target.
//!
//! When one does not, a package that changes only what its player sees is
//! left off with the reason; one that changes the simulation **refuses
//! multiplayer** for that game: the hook installs nothing, as for a build
//! it has no profile for. A file that names a package but cannot be read
//! refuses too, since what it holds cannot be told.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    features::{self, Effect, FeatureSpec},
    index::{Dependency, HookDecl, HookKind, HookUse, Setting},
};

/// The variable naming the enabled packages' file in the game's
/// environment.
pub const ENV: &str = "TPF3MP_NATIVE_MODS";
/// The file's name, in the native-mods folder.
pub const FILE: &str = "enabled.json";
/// The file's format.
pub const FORMAT: u32 = 2;
/// Largest file the hook reads.
pub const MAX_BYTES: u64 = 256 * 1024;

/// Whether rooms compare native mods. They do since protocol 20: the
/// launcher declares the simulation-changing packages it started the game
/// with in the content manifest ([`crate::terms`]), and the room compares
/// them as it compares mods, so no two games of a room run different
/// worlds. Before, such a package refused multiplayer (PLAN.md, Part 3: a
/// channel not checked yet is refused, never used unchecked).
pub const ROOM_TERMS_CARRIED: bool = true;

/// The packages the launcher enabled for one build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enabled {
    pub format: u32,
    /// The game build they are enabled for, by the executable's SHA-256.
    pub build: String,
    pub packages: Vec<EnabledPackage>,
}

/// One enabled package, as installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnabledPackage {
    pub id: String,
    pub version: String,
    /// As the index says; the hook also counts a simulation feature.
    pub simulation: bool,
    pub features: Vec<String>,
    /// The package's defaults with the player's choices over them.
    pub settings: BTreeMap<String, Setting>,
    /// SHA-256 of its sorted `(path, size, sha256)` file manifest; URLs are
    /// excluded because mirrors do not change the installed contents.
    pub files_sha256: String,
    /// Full canonical package identity, including plugins for every
    /// platform. Empty only when reading an older enabled list; such a list
    /// cannot declare a current protocol room term.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub identity_sha256: String,
    /// Package dependencies, which determine native-mod load order.
    pub depends: Vec<Dependency>,
    /// Its folder, `native-mods/<id>/<version>/`.
    pub root: PathBuf,
    /// Its plugins for this platform, in `root`, each with the SHA-256 the
    /// hook checks again before loading it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<EnabledPlugin>,
    /// The game functions its plugins patch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<HookDecl>,
}

/// A plugin the hook loads: its path in the package's folder, its
/// SHA-256, and the plugin interface it was built for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnabledPlugin {
    pub path: String,
    pub sha256: String,
    pub abi: u32,
}

impl EnabledPackage {
    /// Whether it changes the simulation: the index says so, or a feature
    /// it names does (an unknown feature counts as one, fail closed).
    pub fn changes_simulation(&self, registry: &[FeatureSpec]) -> bool {
        self.simulation
            || !self.plugins.is_empty()
            || !self.hooks.is_empty()
            || self.features.iter().any(|id| {
                features::find(registry, id).is_none_or(|f| f.effect == Effect::Simulation)
            })
    }
}

/// A feature the hook switches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Active {
    pub feature: String,
    pub package: String,
    pub version: String,
    pub settings: BTreeMap<String, Setting>,
    pub root: PathBuf,
}

/// What the hook does with the enabled packages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// The features to switch on.
    pub active: Vec<Active>,
    /// The packages whose plugins the hook loads, or whose hooks it patches,
    /// in the load order.
    pub plugins: Vec<EnabledPackage>,
    /// Packages left off, with why: they change only what a player sees.
    pub left_off: Vec<String>,
    /// Why this game must not play in a room, if it must not.
    pub refusal: Option<String>,
}

impl Plan {
    /// The active feature `id`, if the plan switches it on.
    pub fn feature(&self, id: &str) -> Option<&Active> {
        self.active.iter().find(|active| active.feature == id)
    }

    /// The active features that patch the target `name`, in the load
    /// order, with how: the hook merges their values by the rule, or runs
    /// their chains one after the other. They never clash.
    pub fn users<'a>(
        &'a self,
        registry: &'a [FeatureSpec],
        name: &'a str,
    ) -> impl Iterator<Item = (&'a Active, features::Access)> + 'a {
        self.active.iter().filter_map(move |active| {
            features::find(registry, &active.feature)
                .and_then(|spec| spec.access(name))
                .map(|access| (active, access))
        })
    }
}

/// The order packages are switched on in: each after those of `packages`
/// it depends on, the rest by id. `packages` are ids with the ids they
/// depend on; a dependency not among them is not waited for, and packages
/// that depend on each other in a circle come last, by id.
pub fn load_order<'a>(packages: &[(&'a str, Vec<&'a str>)]) -> Vec<&'a str> {
    use std::collections::BTreeSet;
    let present: BTreeSet<&str> = packages.iter().map(|(id, _)| *id).collect();
    let mut waiting: BTreeMap<&str, BTreeSet<&str>> = packages
        .iter()
        .map(|(id, depends)| {
            (
                *id,
                depends
                    .iter()
                    .copied()
                    .filter(|dependency| present.contains(dependency) && dependency != id)
                    .collect(),
            )
        })
        .collect();
    let mut order = Vec::new();
    while let Some(next) = waiting
        .iter()
        .find(|(_, on)| on.is_empty())
        .map(|(id, _)| *id)
    {
        waiting.remove(next);
        for on in waiting.values_mut() {
            on.remove(next);
        }
        order.push(next);
    }
    order.extend(waiting.into_keys());
    order
}

/// Whether two hooks on the same site may both be on: the same kind, both
/// sharing (`chain`), and for a rewrite, the very same bytes given by the
/// package (a plugin's own cannot be known before it starts).
pub fn may_share(a: &HookDecl, b: &HookDecl) -> bool {
    a.kind == b.kind
        && a.access == HookUse::Chain
        && b.access == HookUse::Chain
        && (a.kind != HookKind::Rewrite
            || (a.expected.eq_ignore_ascii_case(&b.expected)
                && a.replacement.is_some()
                && a.replacement.as_deref().map(str::to_ascii_lowercase)
                    == b.replacement.as_deref().map(str::to_ascii_lowercase)))
}

/// Why hooks `mine` and `theirs`, declared by two packages, cannot both be
/// on, if they cannot: they patch overlapping bytes of the same place in
/// the game's code (the same signature, overlapping offsets; a slot, the
/// same entry) and may not share it ([`may_share`]). The hook checks again
/// by address once it has found them.
pub fn hooks_clash(mine: &[HookDecl], theirs: &[HookDecl]) -> Option<String> {
    let signature = |hook: &HookDecl| {
        hook.signature
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase()
    };
    let overlap = |a: &HookDecl, b: &HookDecl| {
        if signature(a) != signature(b) {
            return false;
        }
        match (a.kind, b.kind) {
            (HookKind::Slot, HookKind::Slot) => {
                a.offset == b.offset && a.slot.map(|s| s.index) == b.slot.map(|s| s.index)
            }
            (HookKind::Slot, _) | (_, HookKind::Slot) => false,
            (HookKind::Find, _) | (_, HookKind::Find) => false,
            _ => {
                let end = |h: &HookDecl| h.offset + h.written_len() as i64;
                a.offset < end(b) && b.offset < end(a)
            }
        }
    };
    mine.iter().find_map(|hook| {
        theirs
            .iter()
            .find(|other| {
                overlap(hook, other) && !(hook.offset == other.offset && may_share(hook, other))
            })
            .map(|other| {
                format!(
                    "{} and {} patch the same place in the game, and cannot share it",
                    hook.target, other.target
                )
            })
    })
}

/// Why a package with `features` cannot be on beside `others` (each a
/// package's id and its features), if it cannot: its first clash.
pub fn first_clash(
    registry: &[FeatureSpec],
    features: &[String],
    others: &[(&str, &[String])],
) -> Option<String> {
    features.iter().find_map(|mine| {
        let mine = features::find(registry, mine)?;
        others.iter().find_map(|(package, theirs)| {
            theirs.iter().find_map(|theirs| {
                let theirs = features::find(registry, theirs)?;
                features::clash(mine, theirs).map(|why| format!("it clashes with {package}: {why}"))
            })
        })
    })
}

/// What the hook does with `enabled`, running in the build hashing to
/// `build`, with the features in `registry`, where `has_target` says
/// whether the matched profile has a target.
pub fn plan(
    enabled: &Enabled,
    build: &str,
    registry: &[FeatureSpec],
    has_target: impl Fn(&str) -> bool,
    terms_carried: bool,
) -> Plan {
    let mut plan = Plan::default();
    let mut refusals = Vec::new();
    for package in &enabled.packages {
        let name = format!("{} {}", package.id, package.version);
        let problem = if enabled.format != FORMAT {
            Some(format!("the enabled list has format {}", enabled.format))
        } else if !build.eq_ignore_ascii_case(&enabled.build) {
            Some("it was enabled for another build of the game".to_owned())
        } else {
            package
                .features
                .iter()
                .find_map(|id| match features::find(registry, id) {
                    None => Some(format!("this hook has no feature {id}")),
                    Some(feature) => feature
                        .targets
                        .iter()
                        .find(|target| !has_target(target.name))
                        .map(|target| {
                            format!(
                                "{id} needs {}, which this build's profile lacks",
                                target.name
                            )
                        }),
                })
        };
        let simulation = package.changes_simulation(registry);
        let problem = problem.or_else(|| {
            (simulation && !terms_carried).then(|| {
                "it changes the simulation, and rooms do not compare native mods yet".to_owned()
            })
        });
        // Its features against those already on, and against each other.
        let problem = problem.or_else(|| {
            let mut on: Vec<(&str, Vec<String>)> = Vec::new();
            for active in &plan.active {
                match on.iter_mut().find(|(id, _)| *id == active.package) {
                    Some((_, features)) => features.push(active.feature.clone()),
                    None => on.push((&active.package, vec![active.feature.clone()])),
                }
            }
            let others: Vec<(&str, &[String])> = on
                .iter()
                .map(|(id, features)| (*id, features.as_slice()))
                .collect();
            first_clash(registry, &package.features, &others).or_else(|| {
                package.features.iter().enumerate().find_map(|(at, mine)| {
                    let later = &package.features[at + 1..];
                    first_clash(
                        registry,
                        std::slice::from_ref(mine),
                        &[(&package.id, later)],
                    )
                })
            })
        });
        // Its plugins: built for the interface this hook offers.
        let problem = problem.or_else(|| {
            package
                .plugins
                .iter()
                .find(|plugin| plugin.abi != crate::index::PLUGIN_ABI)
                .map(|plugin| {
                    format!(
                        "its plugin {} is built for plugin interface {}, and this hook offers {}",
                        plugin.path,
                        plugin.abi,
                        crate::index::PLUGIN_ABI
                    )
                })
        });
        match (problem, simulation) {
            (Some(why), true) => refusals.push(format!("{name}: {why}")),
            (Some(why), false) => plan.left_off.push(format!("{name}: {why}")),
            (None, _) if !package.plugins.is_empty() || !package.hooks.is_empty() => {
                plan.plugins.push(package.clone());
                plan.active
                    .extend(package.features.iter().map(|feature| Active {
                        feature: feature.clone(),
                        package: package.id.clone(),
                        version: package.version.clone(),
                        settings: package.settings.clone(),
                        root: package.root.clone(),
                    }));
            }
            (None, _) => plan
                .active
                .extend(package.features.iter().map(|feature| Active {
                    feature: feature.clone(),
                    package: package.id.clone(),
                    version: package.version.clone(),
                    settings: package.settings.clone(),
                    root: package.root.clone(),
                })),
        }
    }
    if !refusals.is_empty() {
        plan.active.clear();
        plan.plugins.clear();
        plan.refusal = Some(format!(
            "native mods that change the simulation cannot run: {}",
            refusals.join("; ")
        ));
    }
    plan
}

/// The enabled packages in the file `get` names under [`ENV`], read, and
/// the hook's plan for them. No variable: nothing enabled. A file that
/// cannot be read refuses multiplayer.
pub fn plan_from_env(
    get: impl Fn(&str) -> Option<String>,
    build: &str,
    registry: &[FeatureSpec],
    has_target: impl Fn(&str) -> bool,
) -> Plan {
    let Some(path) = get(ENV).filter(|path| !path.is_empty()) else {
        return Plan::default();
    };
    match read(Path::new(&path)) {
        Ok(enabled) => plan(&enabled, build, registry, has_target, ROOM_TERMS_CARRIED),
        Err(error) => Plan {
            refusal: Some(format!("the enabled native mods cannot be read: {error}")),
            ..Plan::default()
        },
    }
}

/// Reads an enabled list.
pub fn read(path: &Path) -> Result<Enabled, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{} is too large", path.display()));
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::{
        features::example::BIG_MAPS,
        index::tests::{BUILD, OTHER_BUILD},
    };

    fn enabled(packages: Vec<EnabledPackage>) -> Enabled {
        Enabled {
            format: FORMAT,
            build: BUILD.into(),
            packages,
        }
    }

    fn package(id: &str, simulation: bool, features: &[&str]) -> EnabledPackage {
        EnabledPackage {
            id: id.into(),
            version: "1.0.0".into(),
            simulation,
            features: features.iter().map(|f| (*f).to_owned()).collect(),
            settings: BTreeMap::from([("depth".into(), Setting::Int(10))]),
            files_sha256: crate::terms::files_digest(&[]),
            identity_sha256: String::new(),
            depends: Vec::new(),
            root: PathBuf::from(format!("native-mods/{id}/1.0.0")),
            plugins: Vec::new(),
            hooks: Vec::new(),
        }
    }

    fn every_target(_: &str) -> bool {
        true
    }

    #[test]
    fn a_matching_build_with_every_target_switches_features_on() {
        let list = enabled(vec![package(
            "bigmap",
            true,
            &["bigmap.octree", "bigmap.page"],
        )]);
        let plan = plan(&list, BUILD, BIG_MAPS, every_target, true);
        assert_eq!(plan.refusal, None);
        assert_eq!(
            plan.feature("bigmap.octree").map(|a| &a.settings["depth"]),
            Some(&Setting::Int(10))
        );
        assert!(plan.feature("bigmap.page").is_some());
        assert!(plan.feature("bigmap.density").is_none(), "not enabled");
    }

    #[test]
    fn simulation_packages_run_only_where_rooms_compare_them() {
        let list = enabled(vec![package("bigmap", true, &["bigmap.octree"])]);
        let refused = plan(&list, BUILD, BIG_MAPS, every_target, false);
        assert!(refused.active.is_empty());
        assert!(
            refused
                .refusal
                .unwrap()
                .contains("do not compare native mods")
        );
        // Rooms compare them (protocol 20): they run.
        const { assert!(ROOM_TERMS_CARRIED) };
        let runs = plan(&list, BUILD, BIG_MAPS, every_target, ROOM_TERMS_CARRIED);
        assert_eq!(runs.refusal, None);
        assert!(runs.feature("bigmap.octree").is_some());
    }

    #[test]
    fn another_build_enables_nothing() {
        let mut list = enabled(vec![
            package("bigmap", true, &["bigmap.octree"]),
            package("pages", false, &["bigmap.page"]),
        ]);
        list.build = OTHER_BUILD.into();
        let plan = plan(&list, BUILD, BIG_MAPS, every_target, true);
        assert!(plan.active.is_empty());
        assert!(plan.refusal.is_some(), "the simulation package refuses");
        assert_eq!(plan.left_off.len(), 1, "the page is only left off");
    }

    #[test]
    fn a_missing_target_or_feature_refuses_or_leaves_off() {
        let list = enabled(vec![
            package("bigmap", true, &["bigmap.octree"]),
            package("pages", false, &["bigmap.page"]),
        ]);
        // The profile lacks the page's targets: the page is left off.
        let plan_a = plan(
            &list,
            BUILD,
            BIG_MAPS,
            |target| !target.starts_with("bigmap::size"),
            true,
        );
        assert_eq!(plan_a.refusal, None);
        assert!(plan_a.feature("bigmap.octree").is_some());
        assert!(plan_a.feature("bigmap.page").is_none());
        assert_eq!(plan_a.left_off.len(), 1);
        // It lacks the octree's: no game in a room.
        let plan_b = plan(&list, BUILD, BIG_MAPS, |t| t != "bigmap::octree_root", true);
        assert!(plan_b.refusal.is_some());
        assert!(plan_b.active.is_empty());
        // A hook without the features at all: an unknown feature counts as
        // changing the simulation, even in a package that says it does not.
        let quiet = enabled(vec![package("pages", false, &["bigmap.page"])]);
        let plan_c = plan(&quiet, BUILD, crate::features::BUILT_IN, every_target, true);
        assert!(plan_c.refusal.is_some());
    }

    #[test]
    fn the_load_order_puts_dependencies_first() {
        let order = load_order(&[
            ("zebra", vec![]),
            ("bigmap", vec!["terrain", "absent"]),
            ("terrain", vec!["zebra"]),
            ("alpha", vec![]),
            ("loop_a", vec!["loop_b"]),
            ("loop_b", vec!["loop_a"]),
        ]);
        assert_eq!(
            order,
            ["alpha", "zebra", "terrain", "bigmap", "loop_a", "loop_b"]
        );
    }

    #[test]
    fn features_that_clash_with_one_on_before_do_not_run() {
        use crate::features::{FeatureSpec, Merge, chain, replace, value};
        const REGISTRY: &[FeatureSpec] = &[
            FeatureSpec {
                id: "wide.size",
                effect: Effect::Simulation,
                targets: &[value("map::size", Merge::Max)],
            },
            FeatureSpec {
                id: "wider.size",
                effect: Effect::Simulation,
                targets: &[value("map::size", Merge::Max), chain("sim::step")],
            },
            FeatureSpec {
                id: "steps.own",
                effect: Effect::Simulation,
                targets: &[replace("sim::step")],
            },
            FeatureSpec {
                id: "look.step",
                effect: Effect::Ui,
                targets: &[chain("sim::step")],
            },
            FeatureSpec {
                id: "look.own",
                effect: Effect::Ui,
                targets: &[replace("ui::page")],
            },
            FeatureSpec {
                id: "page.own",
                effect: Effect::Ui,
                targets: &[replace("ui::page")],
            },
        ];
        // Two that merge, and a chain beside them: all on, in order.
        let list = enabled(vec![
            package("wide", true, &["wide.size"]),
            package("wider", true, &["wider.size"]),
            package("look", false, &["look.step"]),
        ]);
        let merged = plan(&list, BUILD, REGISTRY, every_target, true);
        assert_eq!(merged.refusal, None);
        let users: Vec<&str> = merged
            .users(REGISTRY, "map::size")
            .map(|(active, _)| active.package.as_str())
            .collect();
        assert_eq!(users, ["wide", "wider"]);
        assert_eq!(merged.users(REGISTRY, "sim::step").count(), 2);

        // One that takes over what another chains: a simulation package
        // refuses the room; a display-only one is left off.
        let list = enabled(vec![
            package("wider", true, &["wider.size"]),
            package("steps", true, &["steps.own"]),
        ]);
        let refused = plan(&list, BUILD, REGISTRY, every_target, true);
        assert!(refused.active.is_empty());
        let why = refused.refusal.unwrap();
        assert!(why.contains("steps 1.0.0: it clashes with wider"), "{why}");
        let list = enabled(vec![
            package("look", false, &["look.own"]),
            package("page", false, &["page.own"]),
        ]);
        let left = plan(&list, BUILD, REGISTRY, every_target, true);
        assert_eq!(left.refusal, None);
        assert!(left.feature("look.own").is_some(), "the first stays on");
        assert!(left.feature("page.own").is_none());
        assert_eq!(left.left_off.len(), 1);

        // A package whose own features clash does not run either.
        let list = enabled(vec![package("both", false, &["look.own", "page.own"])]);
        assert!(
            plan(&list, BUILD, REGISTRY, every_target, true)
                .active
                .is_empty()
        );

        // What the launcher asks before switching one on.
        let wider = ["wider.size".to_owned()];
        assert!(first_clash(REGISTRY, &["steps.own".into()], &[("wider", &wider)]).is_some());
        assert!(first_clash(REGISTRY, &["wide.size".into()], &[("wider", &wider)]).is_none());
        assert!(
            first_clash(
                crate::features::BUILT_IN,
                &["steps.own".into()],
                &[("wider", &wider)]
            )
            .is_none(),
            "unknown features clash with nothing"
        );
    }

    #[test]
    fn plugins_load_for_their_interface_and_clash_only_when_one_takes_over() {
        let mut plugged = package("pages", true, &[]);
        plugged.plugins = vec![EnabledPlugin {
            path: "plugin/pages.dll".into(),
            sha256: "a".repeat(64),
            abi: crate::index::PLUGIN_ABI,
        }];
        let list = enabled(vec![plugged.clone()]);
        let loads = plan(&list, BUILD, BIG_MAPS, every_target, true);
        assert_eq!(loads.refusal, None);
        assert_eq!(loads.plugins, [plugged.clone()]);
        // Its code changes the simulation, as far as anyone can tell.
        let mut quiet = plugged.clone();
        quiet.simulation = false;
        assert!(quiet.changes_simulation(BIG_MAPS));
        // Built for another interface: the room is refused, nothing loads.
        plugged.plugins[0].abi = crate::index::PLUGIN_ABI + 1;
        let refused = plan(&enabled(vec![plugged]), BUILD, BIG_MAPS, every_target, true);
        assert!(refused.plugins.is_empty());
        assert!(refused.refusal.unwrap().contains("plugin interface"));

        let hook = |target: &str, signature: &str, access: HookUse| HookDecl {
            target: target.into(),
            signature: signature.into(),
            offset: 0,
            expected: "48".into(),
            access,
            kind: HookKind::Detour,
            steal: None,
            replacement: None,
            slot: None,
        };
        let draw = "48 89 5C 24 ? 57 48 83";
        let chained = [hook("a::draw", draw, HookUse::Chain)];
        let chained_too = [hook("b::draw", "48  89 5c 24 ? 57 48 83", HookUse::Chain)];
        let taken = [hook("c::draw", draw, HookUse::Replace)];
        let elsewhere = [hook("d::tick", "40 53 48 83 EC 20 8B D9", HookUse::Replace)];
        assert_eq!(hooks_clash(&chained, &chained_too), None, "chains share");
        // Splices on one site share; a splice and a detour overlapping do
        // not; rewrites share only the very same bytes.
        let kind =
            |target: &str, kind: HookKind, offset: i64, replacement: Option<&str>| HookDecl {
                kind,
                offset,
                steal: (kind == HookKind::Splice).then_some(5),
                replacement: replacement.map(str::to_owned),
                ..hook(target, draw, HookUse::Chain)
            };
        assert_eq!(
            hooks_clash(
                &[kind("a::s", HookKind::Splice, 8, None)],
                &[kind("b::s", HookKind::Splice, 8, None)]
            ),
            None
        );
        assert!(
            hooks_clash(
                &[kind("a::s", HookKind::Splice, 8, None)],
                &[kind("b::d", HookKind::Detour, 0, None)]
            )
            .is_some(),
            "the detour's jump covers the splice's bytes"
        );
        assert_eq!(
            hooks_clash(
                &[kind("a::s", HookKind::Splice, 20, None)],
                &[kind("b::d", HookKind::Detour, 0, None)]
            ),
            None,
            "past the detour's jump"
        );
        assert_eq!(
            hooks_clash(
                &[kind("a::r", HookKind::Rewrite, 2, Some("90"))],
                &[kind("b::r", HookKind::Rewrite, 2, Some("90"))]
            ),
            None
        );
        assert!(
            hooks_clash(
                &[kind("a::r", HookKind::Rewrite, 2, Some("90"))],
                &[kind("b::r", HookKind::Rewrite, 2, Some("CC"))]
            )
            .is_some()
        );
        assert!(
            hooks_clash(
                &[kind("a::r", HookKind::Rewrite, 2, None)],
                &[kind("b::r", HookKind::Rewrite, 2, None)]
            )
            .is_some(),
            "a plugin's own bytes are not known ahead"
        );
        assert!(
            hooks_clash(&chained_too, &taken).is_some(),
            "the same site, spelt apart"
        );
        assert_eq!(hooks_clash(&taken, &elsewhere), None);
    }

    #[test]
    fn the_environment_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let list = enabled(vec![package("pages", false, &["bigmap.page"])]);
        std::fs::write(&path, serde_json::to_vec(&list).unwrap()).unwrap();
        let env = HashMap::from([(ENV, path.to_string_lossy().into_owned())]);
        let plan = plan_from_env(|key| env.get(key).cloned(), BUILD, BIG_MAPS, every_target);
        assert!(plan.feature("bigmap.page").is_some());

        // No variable: nothing at all.
        assert_eq!(
            plan_from_env(|_| None, BUILD, BIG_MAPS, every_target),
            Plan::default()
        );
        // A file that cannot be read refuses.
        std::fs::write(&path, b"{ not json").unwrap();
        let plan = plan_from_env(|key| env.get(key).cloned(), BUILD, BIG_MAPS, every_target);
        assert!(plan.refusal.is_some());
        let gone = HashMap::from([(
            ENV,
            dir.path().join("gone.json").to_string_lossy().into_owned(),
        )]);
        assert!(
            plan_from_env(|key| gone.get(key).cloned(), BUILD, BIG_MAPS, every_target)
                .refusal
                .is_some()
        );
    }
}
