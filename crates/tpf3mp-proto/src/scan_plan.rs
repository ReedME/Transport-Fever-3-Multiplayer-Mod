//! The world-check algorithm is a room setting, never a per-player choice
//! made after joining. Native modes are experimental and remain opt-in.

use serde::{Deserialize, Serialize};

pub const SCAN_MODE_ENV: &str = "TPF3MP_HOOK_NATIVE_LANES";
pub const SCAN_PLAN_ENV: &str = "TPF3MP_HOOK_PARTS";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanMode {
    RollingV1,
    NativeCompareV2,
    NativeV2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanPlan {
    pub mode: ScanMode,
    pub parts: u32,
    pub stride: u32,
}

impl Default for ScanPlan {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl ScanPlan {
    pub const DEFAULT: Self = Self {
        mode: ScanMode::RollingV1,
        parts: 10,
        stride: 1,
    };

    pub fn is_valid(self) -> bool {
        (1..=1024).contains(&self.parts)
            && (1..=1000).contains(&self.stride)
            && (self.mode != ScanMode::RollingV1 || self == Self::DEFAULT)
    }

    /// Parse the launcher's and hook's shared configuration. Off mode has
    /// one canonical plan because partition settings do not affect it.
    pub fn from_values(mode: Option<&str>, plan: Option<&str>) -> Result<Self, &'static str> {
        let mode = match mode.map(str::trim).filter(|s| !s.is_empty()) {
            None => ScanMode::RollingV1,
            Some(s) if s.eq_ignore_ascii_case("off") => ScanMode::RollingV1,
            Some(s) if s.eq_ignore_ascii_case("compare") => ScanMode::NativeCompareV2,
            Some(s) if s.eq_ignore_ascii_case("on") => ScanMode::NativeV2,
            _ => return Err("native scan mode must be off, compare or on"),
        };
        let (parts, stride) = match plan.map(str::trim).filter(|s| !s.is_empty()) {
            None => (10, 1),
            Some(value) => {
                let (parts, stride) = value
                    .split_once(['x', 'X'])
                    .ok_or("native scan plan must be parts x stride (for example 5x10)")?;
                (
                    parts.parse().map_err(|_| "invalid native scan parts")?,
                    stride.parse().map_err(|_| "invalid native scan stride")?,
                )
            }
        };
        let result = Self {
            mode,
            parts,
            stride,
        };
        if !(1..=1024).contains(&parts) || !(1..=1000).contains(&stride) {
            return Err("native scan parts must be 1..1024 and stride 1..1000");
        }
        Ok(if mode == ScanMode::RollingV1 {
            Self::DEFAULT
        } else {
            result
        })
    }

    pub fn from_environment() -> Result<Self, &'static str> {
        Self::from_environment_overrides(&[])
    }

    /// The environment a launcher gives its child: explicit game values
    /// override the inherited process values. Ambiguous duplicates refuse.
    pub fn from_environment_overrides(
        overrides: &[(String, String)],
    ) -> Result<Self, &'static str> {
        fn read(
            name: &str,
            overrides: &[(String, String)],
        ) -> Result<Option<String>, &'static str> {
            let mut matching = overrides.iter().filter(|(key, _)| {
                if cfg!(windows) {
                    key.eq_ignore_ascii_case(name)
                } else {
                    key == name
                }
            });
            if let Some((_, value)) = matching.next() {
                if matching.next().is_some() {
                    return Err("duplicate native scan setting overrides");
                }
                return Ok(Some(value.clone()));
            }
            match std::env::var(name) {
                Ok(value) => Ok(Some(value)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(std::env::VarError::NotUnicode(_)) => Err("native scan settings must be UTF-8"),
            }
        }
        Self::from_values(
            read(SCAN_MODE_ENV, overrides)?.as_deref(),
            read(SCAN_PLAN_ENV, overrides)?.as_deref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_off_are_one_rolling_plan() {
        assert_eq!(ScanPlan::from_values(None, None), Ok(ScanPlan::DEFAULT));
        assert_eq!(
            ScanPlan::from_values(Some("off"), Some("5x10")),
            Ok(ScanPlan::DEFAULT)
        );
    }

    #[test]
    fn effective_game_environment_is_parsed_before_advertising() {
        let overrides = vec![
            (SCAN_MODE_ENV.to_owned(), "compare".to_owned()),
            (SCAN_PLAN_ENV.to_owned(), "5x10".to_owned()),
        ];
        let plan = ScanPlan::from_environment_overrides(&overrides).unwrap();
        assert_eq!(
            plan,
            ScanPlan::from_values(Some("compare"), Some("5x10")).unwrap()
        );
        let mut duplicate = overrides.clone();
        duplicate.push((SCAN_MODE_ENV.to_owned(), "on".to_owned()));
        assert!(ScanPlan::from_environment_overrides(&duplicate).is_err());
        let malformed = vec![(SCAN_MODE_ENV.to_owned(), "onn".to_owned())];
        assert!(ScanPlan::from_environment_overrides(&malformed).is_err());
    }

    #[test]
    fn experimental_modes_have_exact_validated_plans() {
        let plan = ScanPlan::from_values(Some("on"), Some("5x10")).unwrap();
        assert_eq!(
            plan,
            ScanPlan {
                mode: ScanMode::NativeV2,
                parts: 5,
                stride: 10
            }
        );
        assert!(plan.is_valid());
        assert_ne!(
            plan,
            ScanPlan::from_values(Some("compare"), Some("5x10")).unwrap()
        );
        for value in ["0x1", "1025x1", "1x0", "1x1001", "5", "5x10x1"] {
            assert!(
                ScanPlan::from_values(Some("on"), Some(value)).is_err(),
                "{value}"
            );
        }
        assert!(ScanPlan::from_values(Some("onn"), None).is_err());
        assert!(
            !ScanPlan {
                parts: 5,
                ..ScanPlan::DEFAULT
            }
            .is_valid()
        );
    }
}
