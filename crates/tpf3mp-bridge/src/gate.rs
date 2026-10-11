//! The hook's side of the step gate.

use thiserror::Error;
use tpf3mp_proto::{ChatText, Event, IntentRejection, Payload, PlayerId, Speed, Text};

use crate::{LobbyView, MAX_PATH, RoomInfo, ToHook};

/// Stands before every step the game runs. The game may run its next step
/// once the agent has released it; until then the hook reads messages and
/// the gate hands over the events to apply first.
///
/// Anything out of order is an error. The hook must then stop following the
/// room and say so, rather than guess.
///
/// ```text
/// before each step:
///     while !gate.may_run() {
///         match gate.on_message(next message)? {
///             Gated::Apply(event) => apply it to the world,
///             ...
///         }
///     }
///     run the step, then gate.ran() and report ToAgent::Ran
/// ```
#[derive(Debug, Clone)]
pub struct Gate {
    next_step: u64,
    released: u64,
    ended: bool,
    /// A load was ordered and the world is not loaded yet.
    loading: bool,
}

/// What the game should do with a message read at the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gated {
    /// Apply this event now, before the next step.
    Apply(Event),
    /// Show the room's new speed.
    Speed(Speed),
    /// Tell the player this replica diverged.
    Diverged { step: u64, lanes: Vec<u16> },
    /// Tell the player one of their commands was refused.
    Refused {
        command: u64,
        reason: IntentRejection,
    },
    /// The session is over; stop waiting at the gate.
    Ended(Text<128>),
    /// Load this world, then call [`Gate::loaded`].
    Load {
        file: Option<Text<MAX_PATH>>,
        next_step: u64,
        world_epoch: u64,
    },
    /// Show a chat message.
    Chat { from: Text<32>, text: ChatText },
    /// Show the room as it stands.
    Room(RoomInfo),
    /// Keep the launcher's lobby for the main menu's window.
    Lobby(Box<LobbyView>),
    /// Show what another member's build tool shows.
    Preview {
        from: PlayerId,
        preview: Option<Payload>,
    },
    /// Nothing to do but check [`Gate::may_run`] again.
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GateError {
    #[error("an event for step {got} arrived while the game waits before step {expected}")]
    EventStep { expected: u64, got: u64 },
    #[error("an event arrived after step {step} was already released")]
    EventAfterRelease { step: u64 },
    #[error("the release went back from step {from} to {to}")]
    ReleaseBackwards { from: u64, to: u64 },
    #[error("{0} has no place at the gate")]
    Unexpected(&'static str),
    #[error("a message arrived while the world loads")]
    Loading,
    #[error("the step counter is at the end of its range")]
    Overflow,
}

impl Gate {
    /// A gate before `next_step`, the first step the game will run: 1 for
    /// a new game.
    pub fn new(next_step: u64) -> Self {
        Self {
            next_step,
            released: next_step.saturating_sub(1),
            ended: false,
            loading: false,
        }
    }

    /// The world ordered by the last [`ToHook::Load`] is loaded, and runs
    /// `next_step` next. Nothing is released for it yet.
    pub fn loaded(&mut self, next_step: u64) {
        self.next_step = next_step;
        self.released = next_step.saturating_sub(1);
        self.loading = false;
    }

    /// Whether a load was ordered that has not finished.
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// The step the game runs next.
    pub fn next_step(&self) -> u64 {
        self.next_step
    }

    /// The last step released: the step before the next while none is.
    pub fn released(&self) -> u64 {
        self.released
    }

    /// The steps from the next that are released: 0 when the next is not.
    pub fn released_ahead(&self) -> u64 {
        if self.may_run() {
            self.released - self.next_step + 1
        } else {
            0
        }
    }

    /// How many steps from the next the game may run as one batch, at most
    /// `max`: the steps released so far (no event can fall between them: an
    /// event for a released step is refused), ending at the first checkpoint
    /// step, whose lanes must be the world's right after it. 0 when the next
    /// step is not released.
    pub fn batch(&self, max: u64, checkpoint_interval: u64) -> u64 {
        if !self.may_run() {
            return 0;
        }
        let released = self.released_ahead();
        let interval = checkpoint_interval.max(1);
        let to_checkpoint = (interval - self.next_step % interval) % interval + 1;
        released.min(to_checkpoint).min(max)
    }

    /// Whether the game may run its next step now.
    pub fn may_run(&self) -> bool {
        !self.ended && !self.loading && self.released >= self.next_step
    }

    /// Whether the session has ended.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Handles one message read while the game waits before its next step.
    pub fn on_message(&mut self, message: ToHook) -> Result<Gated, GateError> {
        // The lobby is for the menu's window and changes nothing here: it
        // may come at any time, a load included. So may another member's
        // preview, which a world on its way has nowhere to show.
        if self.loading && matches!(message, ToHook::Preview { .. }) {
            return Ok(Gated::Nothing);
        }
        if self.loading && !matches!(message, ToHook::End { .. } | ToHook::Lobby(_)) {
            return Err(GateError::Loading);
        }
        match message {
            ToHook::Apply(event) => {
                if event.step != self.next_step {
                    return Err(GateError::EventStep {
                        expected: self.next_step,
                        got: event.step,
                    });
                }
                if self.released >= self.next_step {
                    return Err(GateError::EventAfterRelease { step: event.step });
                }
                Ok(Gated::Apply(event))
            }
            ToHook::Release { through } => {
                if through < self.released {
                    return Err(GateError::ReleaseBackwards {
                        from: self.released,
                        to: through,
                    });
                }
                self.released = through;
                Ok(Gated::Nothing)
            }
            ToHook::Speed(speed) => Ok(Gated::Speed(speed)),
            ToHook::Diverged { step, lanes } => Ok(Gated::Diverged { step, lanes }),
            ToHook::Refused { command, reason } => Ok(Gated::Refused { command, reason }),
            ToHook::End { reason } => {
                self.ended = true;
                Ok(Gated::Ended(reason))
            }
            ToHook::Load {
                file,
                next_step,
                world_epoch,
            } => {
                self.loading = true;
                Ok(Gated::Load {
                    file,
                    next_step,
                    world_epoch,
                })
            }
            ToHook::Chat { from, text } => Ok(Gated::Chat { from, text }),
            ToHook::Room(room) => Ok(Gated::Room(room)),
            ToHook::Lobby(view) => Ok(Gated::Lobby(view)),
            ToHook::Preview { from, preview } => Ok(Gated::Preview { from, preview }),
            ToHook::Hello { .. } => Err(GateError::Unexpected("a hello")),
            ToHook::Begin { .. } => Err(GateError::Unexpected("the start of a game")),
        }
    }

    /// Records that the game ran its next step, and returns that step.
    pub fn ran(&mut self) -> Result<u64, GateError> {
        let step = self.next_step;
        self.next_step = step.checked_add(1).ok_or(GateError::Overflow)?;
        Ok(step)
    }
}

#[cfg(test)]
mod tests {
    use tpf3mp_proto::{EventBody, FixedBytes, PlayerId};

    use super::*;

    fn event(seq: u64, step: u64) -> Event {
        Event {
            seq,
            step,
            body: EventBody::PlayerLeft {
                player: PlayerId(FixedBytes([1; 32])),
                kicked: false,
            },
        }
    }

    #[test]
    fn events_apply_between_the_right_steps() {
        let mut gate = Gate::new(1);
        assert!(!gate.may_run(), "nothing released yet");
        assert_eq!(
            gate.on_message(ToHook::Apply(event(1, 1))),
            Ok(Gated::Apply(event(1, 1)))
        );
        gate.on_message(ToHook::Release { through: 3 }).unwrap();
        assert!(gate.may_run());
        assert_eq!(gate.ran(), Ok(1));
        assert_eq!(gate.ran(), Ok(2));
        assert_eq!(gate.ran(), Ok(3));
        assert!(!gate.may_run(), "step 4 is not released");
        gate.on_message(ToHook::Apply(event(2, 4))).unwrap();
        gate.on_message(ToHook::Release { through: 4 }).unwrap();
        assert_eq!(gate.ran(), Ok(4));
    }

    #[test]
    fn a_batch_is_the_released_steps_up_to_a_checkpoint() {
        let mut gate = Gate::new(1);
        assert_eq!(gate.batch(16, 50), 0, "nothing released");
        gate.on_message(ToHook::Release { through: 30 }).unwrap();
        assert_eq!(gate.batch(16, 50), 16, "at most max");
        assert_eq!(gate.batch(64, 50), 30, "the released steps");
        assert_eq!(gate.batch(64, 10), 10, "ending at checkpoint step 10");
        for _ in 0..10 {
            gate.ran().unwrap();
        }
        assert_eq!(gate.batch(64, 10), 10, "steps 11 to 20");
        gate.ran().unwrap();
        assert_eq!(gate.batch(64, 10), 9, "steps 12 to 20");
        assert_eq!(gate.batch(64, 1), 1, "every step a checkpoint");
        assert_eq!(gate.batch(64, 0), 1, "an interval of 0 is taken as 1");
    }

    #[test]
    fn an_event_for_another_step_is_refused() {
        let mut gate = Gate::new(5);
        assert_eq!(
            gate.on_message(ToHook::Apply(event(1, 6))),
            Err(GateError::EventStep {
                expected: 5,
                got: 6
            })
        );
        assert_eq!(
            gate.on_message(ToHook::Apply(event(1, 4))),
            Err(GateError::EventStep {
                expected: 5,
                got: 4
            })
        );
    }

    #[test]
    fn an_event_for_a_released_step_is_refused() {
        let mut gate = Gate::new(1);
        gate.on_message(ToHook::Release { through: 1 }).unwrap();
        assert_eq!(
            gate.on_message(ToHook::Apply(event(1, 1))),
            Err(GateError::EventAfterRelease { step: 1 })
        );
    }

    #[test]
    fn releases_never_go_back() {
        let mut gate = Gate::new(1);
        gate.on_message(ToHook::Release { through: 5 }).unwrap();
        assert_eq!(
            gate.on_message(ToHook::Release { through: 4 }),
            Err(GateError::ReleaseBackwards { from: 5, to: 4 })
        );
        assert_eq!(
            gate.on_message(ToHook::Release { through: 5 }),
            Ok(Gated::Nothing),
            "repeating a release is harmless"
        );
    }

    #[test]
    fn the_end_stops_the_game_waiting() {
        let mut gate = Gate::new(1);
        gate.on_message(ToHook::Release { through: 9 }).unwrap();
        let reason = Text::new("the room closed").unwrap();
        assert_eq!(
            gate.on_message(ToHook::End {
                reason: reason.clone()
            }),
            Ok(Gated::Ended(reason))
        );
        assert!(gate.ended());
        assert!(!gate.may_run());
    }

    #[test]
    fn a_load_voids_the_old_world_until_the_new_one_is_loaded() {
        let mut gate = Gate::new(1);
        gate.on_message(ToHook::Release { through: 9 }).unwrap();
        let file = Text::new("saves/save-40.sav").unwrap();
        assert_eq!(
            gate.on_message(ToHook::Load {
                world_epoch: 1,
                file: Some(file.clone()),
                next_step: 300
            }),
            Ok(Gated::Load {
                world_epoch: 1,
                file: Some(file),
                next_step: 300
            })
        );
        assert!(gate.loading());
        assert!(!gate.may_run(), "nothing runs while the world loads");
        assert_eq!(
            gate.on_message(ToHook::Release { through: 400 }),
            Err(GateError::Loading)
        );
        gate.loaded(300);
        assert!(!gate.loading());
        assert!(!gate.may_run(), "the new world has nothing released yet");
        gate.on_message(ToHook::Apply(event(7, 300))).unwrap();
        gate.on_message(ToHook::Release { through: 300 }).unwrap();
        assert_eq!(gate.ran(), Ok(300));
    }

    #[test]
    fn a_preview_shows_in_a_world_that_plays_and_is_dropped_while_one_loads() {
        let mut gate = Gate::new(1);
        let from = PlayerId(FixedBytes([2; 32]));
        let preview = Some(tpf3mp_proto::Payload::new(vec![1, 2]).unwrap());
        assert_eq!(
            gate.on_message(ToHook::Preview {
                from,
                preview: preview.clone()
            }),
            Ok(Gated::Preview {
                from,
                preview: preview.clone()
            })
        );
        gate.on_message(ToHook::Load {
            world_epoch: 1,
            file: None,
            next_step: 1,
        })
        .unwrap();
        assert_eq!(
            gate.on_message(ToHook::Preview { from, preview }),
            Ok(Gated::Nothing),
            "advisory: never a reason to give up on the room"
        );
        assert!(gate.loading());
    }

    #[test]
    fn a_resumed_world_starts_where_it_stands() {
        let mut gate = Gate::new(101);
        assert!(!gate.may_run());
        gate.on_message(ToHook::Release { through: 101 }).unwrap();
        assert_eq!(gate.ran(), Ok(101));
    }
}
