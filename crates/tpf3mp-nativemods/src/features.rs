//! The features built into TPF3-MP's hook, by id: what a native package can
//! switch on.
//!
//! In this first version a native package carries no code of its own. It
//! names features already compiled into the hook, which the hook switches on
//! with the package's settings only when the launcher enabled the package
//! and the running build's profile has every target the feature patches
//! ([`crate::enabled`]). The launcher reads the same registry to refuse a
//! package this TPF3-MP cannot run before downloading it.
//!
//! [`BUILT_IN`] is empty on `dev`: no native feature is built into the hook
//! yet. Big Maps, on its own branches, would add its features here; the
//! shape it would take is [`example::BIG_MAPS`], which no hook runs.
//!
//! **Several features on one target.** Every patch is the hook's own code,
//! so two packages never patch a game function apart: each feature says how
//! it uses each target it patches ([`Access`]), and the hook combines
//! them, as Harmony combines patches in modded Unity games:
//!
//! - [`Access::Value`]: it sets a number or a list the game reads. Several
//!   combine by the target's [`Merge`] rule: the largest, the smallest, the
//!   sum, or every list's entries together.
//! - [`Access::Chain`]: it runs before or after the game's function and
//!   leaves the function in place. Several all run, in the load order
//!   ([`crate::enabled::load_order`]), the same on every player's game.
//! - [`Access::Replace`]: it takes the function over. It cannot share the
//!   target: a second feature on it [`clash`]es, and the launcher will not
//!   switch on a package whose features clash with one already on.
//!
//! Features that share a target must agree on how they combine: both
//! values with the same rule, or both chains ([`registry_problems`], which
//! the tests hold [`BUILT_IN`] to).

/// What a feature changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Only what one player sees or may do on their own machine: a game
    /// may run without it.
    Ui,
    /// How the world runs: every game of a room runs it alike, or none.
    Simulation,
}

/// How a feature uses a target it patches, and so how it shares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// It sets a value the game reads; several merge by the rule.
    Value(Merge),
    /// It runs before or after the game's function; several all run, in
    /// the load order.
    Chain,
    /// It takes the function over; nothing else may use the target.
    Replace,
}

/// How the values several features set on one target become one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merge {
    /// The largest number.
    Max,
    /// The smallest number.
    Min,
    /// The numbers added, saturating.
    Sum,
    /// Every list's entries, each once, in order.
    Union,
}

impl Merge {
    /// `values` merged, for a number rule; none for [`Merge::Union`] or no
    /// values.
    pub fn numbers(self, values: &[i64]) -> Option<i64> {
        let mut values = values.iter().copied();
        let first = values.next()?;
        match self {
            Merge::Max => Some(values.fold(first, i64::max)),
            Merge::Min => Some(values.fold(first, i64::min)),
            Merge::Sum => Some(values.fold(first, i64::saturating_add)),
            Merge::Union => None,
        }
    }

    /// `lists` merged, for [`Merge::Union`]: every entry once, sorted.
    pub fn lists<T: Ord + Clone>(self, lists: &[Vec<T>]) -> Option<Vec<T>> {
        (self == Merge::Union).then(|| {
            let mut all: Vec<T> = lists.iter().flatten().cloned().collect();
            all.sort();
            all.dedup();
            all
        })
    }
}

/// A hook-profile target a feature patches, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// Its name in the hook profiles (docs/HOOKS.md).
    pub name: &'static str,
    pub access: Access,
}

/// A target whose value the feature sets, merged by `merge`.
pub const fn value(name: &'static str, merge: Merge) -> Target {
    Target {
        name,
        access: Access::Value(merge),
    }
}

/// A target the feature runs before or after.
pub const fn chain(name: &'static str) -> Target {
    Target {
        name,
        access: Access::Chain,
    }
}

/// A target the feature takes over.
pub const fn replace(name: &'static str) -> Target {
    Target {
        name,
        access: Access::Replace,
    }
}

/// A feature compiled into the hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureSpec {
    /// Ids joined by dots: the package's id, then the feature's.
    pub id: &'static str,
    pub effect: Effect,
    /// The hook-profile targets it patches, all of them, each with how.
    pub targets: &'static [Target],
}

impl FeatureSpec {
    /// How it uses the target `name`, if it patches it.
    pub fn access(&self, name: &str) -> Option<Access> {
        self.targets
            .iter()
            .find(|target| target.name == name)
            .map(|target| target.access)
    }
}

/// Why `a` and `b` cannot both be on, if they cannot: a target both patch
/// that one takes over, or that they would combine differently.
pub fn clash(a: &FeatureSpec, b: &FeatureSpec) -> Option<String> {
    a.targets.iter().find_map(|target| {
        let theirs = b.access(target.name)?;
        match (target.access, theirs) {
            (Access::Chain, Access::Chain) => None,
            (Access::Value(one), Access::Value(other)) if one == other => None,
            (Access::Replace, _) | (_, Access::Replace) => Some(format!(
                "{} and {} both patch {}, and one takes it over",
                a.id, b.id, target.name
            )),
            _ => Some(format!(
                "{} and {} would combine {} differently",
                a.id, b.id, target.name
            )),
        }
    })
}

/// Whether `a` and `b` share a target one of them takes over.
fn takes_over_shared(a: &FeatureSpec, b: &FeatureSpec) -> bool {
    a.targets.iter().any(|target| {
        b.access(target.name)
            .is_some_and(|theirs| target.access == Access::Replace || theirs == Access::Replace)
    })
}

/// What is wrong with how `registry`'s features declare their targets: a
/// target one lists twice, or one that features share but would combine
/// differently. Features that share a target one of them takes over are
/// not wrong: they are only never on together.
pub fn registry_problems(registry: &[FeatureSpec]) -> Vec<String> {
    let mut problems = Vec::new();
    for (at, feature) in registry.iter().enumerate() {
        for (i, target) in feature.targets.iter().enumerate() {
            if feature.targets[..i].iter().any(|t| t.name == target.name) {
                problems.push(format!("{} lists {} twice", feature.id, target.name));
            }
        }
        for other in &registry[at + 1..] {
            if let Some(why) = clash(feature, other)
                && !takes_over_shared(feature, other)
            {
                problems.push(why);
            }
        }
    }
    problems
}

/// The features this hook has. None yet.
pub const BUILT_IN: &[FeatureSpec] = &[];

/// The feature `id` in `registry`.
pub fn find<'a>(registry: &'a [FeatureSpec], id: &str) -> Option<&'a FeatureSpec> {
    registry.iter().find(|feature| feature.id == id)
}

/// Feature registries no hook carries, for the docs and the tests.
pub mod example {
    use super::{Effect, FeatureSpec, Merge, replace, value};

    /// Big Maps (docs/BIGMAPS.md, `crates/tpf3mp-bigmap`) as a native
    /// package's features. Not built into the hook: Big Maps lives on its
    /// own branches.
    pub const BIG_MAPS: &[FeatureSpec] = &[
        FeatureSpec {
            // The octree's root box past ±32,768 m.
            id: "bigmap.octree",
            effect: Effect::Simulation,
            targets: &[value("bigmap::octree_root", Merge::Max)],
        },
        FeatureSpec {
            // The street raster's 32-bit cell count (180 tiles).
            id: "bigmap.street_raster",
            effect: Effect::Simulation,
            targets: &[replace("bigmap::street_raster")],
        },
        FeatureSpec {
            // Town and industry placement attempts.
            id: "bigmap.placement",
            effect: Effect::Simulation,
            targets: &[value("bigmap::placement_attempts", Merge::Max)],
        },
        FeatureSpec {
            // The New Game page with the added sizes, served by the hook.
            id: "bigmap.page",
            effect: Effect::Ui,
            targets: &[
                value("bigmap::tile_count", Merge::Max),
                value("bigmap::size_list", Merge::Union),
            ],
        },
        FeatureSpec {
            // Density levels for towns and industries.
            id: "bigmap.density",
            effect: Effect::Simulation,
            targets: &[value("bigmap::density_levels", Merge::Union)],
        },
        FeatureSpec {
            // Refuses a size this PC lacks the memory to generate; changes
            // nothing in a world.
            id: "bigmap.memory_gate",
            effect: Effect::Ui,
            targets: &[],
        },
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::is_feature_id;

    #[test]
    fn feature_ids_are_well_formed_and_unique() {
        for registry in [BUILT_IN, example::BIG_MAPS] {
            let mut ids: Vec<&str> = registry.iter().map(|f| f.id).collect();
            assert!(ids.iter().all(|id| is_feature_id(id)));
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), registry.len());
        }
        assert_eq!(
            find(example::BIG_MAPS, "bigmap.octree").map(|f| f.effect),
            Some(Effect::Simulation)
        );
        assert!(find(BUILT_IN, "bigmap.octree").is_none());
    }

    /// The check CI holds the hook to: features that share a target agree
    /// on how they combine.
    #[test]
    fn built_in_features_agree_on_shared_targets() {
        assert_eq!(registry_problems(BUILT_IN), Vec::<String>::new());
        assert_eq!(registry_problems(example::BIG_MAPS), Vec::<String>::new());
    }

    #[test]
    fn features_on_one_target_combine_or_clash() {
        const fn spec(id: &'static str, targets: &'static [Target]) -> FeatureSpec {
            FeatureSpec {
                id,
                effect: Effect::Simulation,
                targets,
            }
        }
        const WIDER: FeatureSpec = spec("a.wider", &[value("map::size", Merge::Max)]);
        const WIDER_TOO: FeatureSpec = spec("b.wider", &[value("map::size", Merge::Max)]);
        const SMALLER: FeatureSpec = spec("c.smaller", &[value("map::size", Merge::Min)]);
        const LOG: FeatureSpec = spec("d.log", &[chain("sim::step")]);
        const LOG_TOO: FeatureSpec = spec(
            "e.log",
            &[chain("sim::step"), value("map::size", Merge::Max)],
        );
        const OWN_STEP: FeatureSpec = spec("f.step", &[replace("sim::step")]);
        assert_eq!(clash(&WIDER, &WIDER_TOO), None, "the same rule merges");
        assert_eq!(clash(&LOG, &LOG_TOO), None, "chains both run");
        assert_eq!(clash(&WIDER, &LOG), None, "nothing shared");
        assert!(clash(&WIDER, &SMALLER).unwrap().contains("differently"));
        assert!(clash(&LOG, &OWN_STEP).unwrap().contains("takes it over"));
        assert!(clash(&OWN_STEP, &LOG).is_some(), "either way round");

        // A registry may hold features that take the same target over:
        // they are just never on together. Disagreeing rules are a mistake.
        let registry = [WIDER, LOG, OWN_STEP];
        assert_eq!(registry_problems(&registry), Vec::<String>::new());
        const TWICE: FeatureSpec = spec("g.twice", &[chain("x"), chain("x")]);
        let registry = [WIDER, SMALLER, TWICE];
        assert_eq!(registry_problems(&registry).len(), 2);

        assert_eq!(Merge::Max.numbers(&[3, 9, 4]), Some(9));
        assert_eq!(Merge::Min.numbers(&[3, 9, 4]), Some(3));
        assert_eq!(Merge::Sum.numbers(&[i64::MAX, 1]), Some(i64::MAX));
        assert_eq!(Merge::Max.numbers(&[]), None);
        assert_eq!(Merge::Union.numbers(&[1]), None);
        assert_eq!(
            Merge::Union.lists(&[vec![4, 8], vec![8, 16]]),
            Some(vec![4, 8, 16])
        );
        assert_eq!(Merge::Max.lists(&[vec![1]]), None);
    }
}
