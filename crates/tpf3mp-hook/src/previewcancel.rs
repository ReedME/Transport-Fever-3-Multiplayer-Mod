//! Withdraw a street or track preview when the player aborts the proposal
//! without closing its tool. Build 40408's IA_ABORT handler (0x58d100)
//! reaches the builder's reset (0x576330), which clears its proposal and
//! renderer. The active-tool list stays unchanged, so Lua cannot infer this
//! cancellation from that list. The reset also handles completed builds.
//!
//! Only advisory preview state changes here. The game's reset still runs
//! with its original arguments; a new proposal may publish a new preview.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::atomic::{AtomicUsize, Ordering};

pub use crate::build_data::native::previewcancel::RESET_TARGET;
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
type Reset = unsafe extern "C-unwind" fn(usize, u8);

unsafe extern "C-unwind" fn reset_detour(builder: usize, all: u8) {
    if crate::lua::in_room() {
        crate::previews::withdraw_network();
        // The build's own reset precedes its Add callback; a later reset
        // cancels a kept continuation even when the tool remains open.
        crate::toolchain::proposal_reset(builder);
    }
    // SAFETY: the profile-checked reset's trampoline, with the game's
    // original arguments. Clear first so a new proposal from the reset
    // can publish its own preview afterwards.
    unsafe {
        std::mem::transmute::<usize, Reset>(ORIGINAL.load(Ordering::Acquire))(builder, all);
    }
}

/// Install before any game tool runs.
///
/// # Safety
///
/// `at` resolves checked targets in this game's image and `detour` keeps
/// its trampoline alive, as `InlineDetour::install` requires.
#[cfg(all(windows, target_arch = "x86_64"))]
pub unsafe fn install(
    at: &dyn Fn(&str) -> Result<usize, String>,
    detour: unsafe fn(*mut u8, *const u8) -> Result<usize, String>,
) -> String {
    let result = at(RESET_TARGET).and_then(|address| {
        // SAFETY: the caller's; this reset does not run during bootstrap.
        unsafe { detour(address as *mut u8, reset_detour as *const u8) }
    });
    match result {
        Ok(original) => {
            ORIGINAL.store(original, Ordering::Release);
            crate::toolchain::set_reset_hooked(true);
            "build previews: street and track proposal resets withdraw the player's preview".into()
        }
        Err(why) => format!("build previews: proposal reset withdrawal unavailable: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tpf3mp_proto::{BoundedVec, Payload, Text, action::*};

    fn network(track: bool) -> Payload {
        let polyline = Polyline::new(
            BoundedVec::new(vec![
                Vertex {
                    pos: Pos { x: 0, y: 0, z: 0 },
                    resolve: Resolve::New,
                },
                Vertex {
                    pos: Pos {
                        x: 1000,
                        y: 0,
                        z: 0,
                    },
                    resolve: Resolve::New,
                },
            ])
            .unwrap(),
            BoundedVec::new(vec![Link {
                from: 0,
                to: 1,
                tangent0: Tangent { x: 1, y: 0, z: 0 },
                tangent1: Tangent { x: 1, y: 0, z: 0 },
                structure: Structure::Ground,
                kind: None,
                decorations: BoundedVec::default(),
                locked: false,
                owned: false,
                lanes: BoundedVec::default(),
                precedence: None,
                position0: None,
                position1: None,
                kept: BoundedVec::default(),
            }])
            .unwrap(),
            BoundedVec::default(),
        )
        .unwrap();
        let action = if track {
            Action::BuildTrack(TrackBuild {
                track: Text::new("track/standard.lua").unwrap(),
                style: None,
                catenary: false,
                polyline,
                crossing_type: None,
            })
        } else {
            Action::BuildRoad(RoadBuild {
                street: Text::new("street/standard.lua").unwrap(),
                style: None,
                bus_lane: false,
                tram: Tram::None,
                polyline,
                crossing_type: None,
            })
        };
        action.to_payload().unwrap()
    }

    struct Builder {
        reset: Option<u8>,
        next: Option<tpf3mp_proto::Payload>,
    }

    unsafe extern "C-unwind" fn original(builder: usize, all: u8) {
        // SAFETY: each test hands its own live Builder to the reset.
        let builder = unsafe { &mut *(builder as *mut Builder) };
        builder.reset = Some(all);
        if let Some(next) = builder.next.take() {
            crate::previews::show(Some(next));
        }
    }

    #[test]
    fn aborting_with_the_tool_still_open_withdraws_the_preview_once() {
        let _serial = crate::lua::tests::SERIAL
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        crate::previews::clear();
        crate::lua::set_in_room(true);
        ORIGINAL.store(original as *const () as usize, Ordering::Release);
        for track in [false, true] {
            crate::previews::clear();
            let action = network(track);
            crate::previews::show(Some(action));
            let now = Instant::now();
            assert!(crate::previews::take_out(now).unwrap().is_some());
            let mut builder = Builder {
                reset: None,
                next: None,
            };
            unsafe { reset_detour((&mut builder as *mut Builder) as usize, 1) };
            assert_eq!(builder.reset, Some(1), "the game's reset still ran");
            assert_eq!(
                crate::previews::take_out(now + crate::previews::MIN_INTERVAL),
                Some(None)
            );
            assert_eq!(
                crate::previews::take_out(now + crate::previews::KEEPALIVE),
                None
            );
        }
        crate::lua::set_in_room(false);
        crate::previews::clear();
    }

    #[test]
    fn reset_keeps_plain_games_and_a_new_proposal_published_by_the_game() {
        let _serial = crate::lua::tests::SERIAL
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        crate::previews::clear();
        ORIGINAL.store(original as *const () as usize, Ordering::Release);
        let action = network(false);
        crate::lua::set_in_room(false);
        crate::previews::show(Some(action.clone()));
        let mut builder = Builder {
            reset: None,
            next: None,
        };
        unsafe { reset_detour((&mut builder as *mut Builder) as usize, 0) };
        assert_eq!(builder.reset, Some(0));
        let now = Instant::now();
        assert_eq!(crate::previews::take_out(now), Some(Some(action.clone())));
        crate::lua::set_in_room(true);
        builder.next = Some(action.clone());
        unsafe { reset_detour((&mut builder as *mut Builder) as usize, 1) };
        assert_eq!(
            crate::previews::take_out(now + crate::previews::MIN_INTERVAL),
            Some(Some(action))
        );
        crate::lua::set_in_room(false);
        crate::previews::clear();
    }

    #[test]
    fn an_inactive_network_builder_does_not_withdraw_a_construction_preview() {
        let _serial = crate::lua::tests::SERIAL
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        crate::previews::clear();
        crate::lua::set_in_room(true);
        ORIGINAL.store(original as *const () as usize, Ordering::Release);
        let action = crate::lua::tests::depot_build().to_payload().unwrap();
        crate::previews::show(Some(action.clone()));
        let mut builder = Builder {
            reset: None,
            next: None,
        };
        unsafe { reset_detour((&mut builder as *mut Builder) as usize, 1) };
        assert_eq!(builder.reset, Some(1));
        assert_eq!(
            crate::previews::take_out(Instant::now()),
            Some(Some(action))
        );
        crate::lua::set_in_room(false);
        crate::previews::clear();
    }
}
