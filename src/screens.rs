//! Who holds a screen, and who is driving it.
//!
//! A screen outlives the call that took it, which makes the release the
//! dangerous operation: a slow holder releasing late tears down a screen its
//! replacement is already using. Leases are fenced, so a release that cannot
//! prove it still holds the screen is refused.
//!
//! [`ControlGate`] is the other half: while a person drives a screen, the
//! owner may read it and may not act on it.

use crate::error::{Error, Result};
use crate::{Control, HolderId, ScreenId};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// The default a caller gets when it does not choose.
///
/// Long enough that a slow turn does not lose its screen mid-thought, short
/// enough that a crashed holder does not block one until somebody notices.
pub const DEFAULT_LEASE: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenLease {
    pub screen: ScreenId,
    pub holder: HolderId,
    /// Monotonic per holder. A later fence takes the screen; an earlier one
    /// cannot release it.
    pub fence: u64,
    pub expires_at: SystemTime,
}

impl ScreenLease {
    pub fn expired(&self, now: SystemTime) -> bool {
        now >= self.expires_at
    }
}

/// The screens one box has, and who holds them.
pub struct Screens {
    max: u32,
    held: Mutex<HashMap<ScreenId, ScreenLease>>,
}

impl Screens {
    pub fn new(max: u32) -> Self {
        Self {
            max,
            held: Mutex::new(HashMap::new()),
        }
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<ScreenId, ScreenLease>>> {
        self.held
            .lock()
            .map_err(|_| Error::transport("the screen registry was poisoned by a panic", false))
    }

    /// How many are held right now.
    ///
    /// Expired leases are not counted: a screen whose holder never came back
    /// is free.
    pub fn in_use(&self, now: SystemTime) -> u32 {
        self.lock()
            .map(|held| held.values().filter(|lease| !lease.expired(now)).count() as u32)
            .unwrap_or(0)
    }

    /// Take any free screen, or renew the one this holder already has.
    pub fn claim(
        &self,
        holder: &HolderId,
        fence: u64,
        now: SystemTime,
        ttl: Duration,
    ) -> Result<ScreenLease> {
        let mut held = self.lock()?;

        // A holder that returns gets the screen it was already on, with its
        // windows and browser profile still on it.
        if let Some(existing) = held
            .values()
            .find(|lease| &lease.holder == holder && !lease.expired(now))
            .cloned()
        {
            if fence < existing.fence {
                return Err(Error::ScreenUnavailable {
                    screen: Some(existing.screen),
                    held_by: Some(existing.holder),
                });
            }
            let renewed = ScreenLease {
                fence,
                expires_at: now + ttl,
                ..existing
            };
            held.insert(renewed.screen, renewed.clone());
            return Ok(renewed);
        }

        for index in 0..self.max {
            let screen = ScreenId(index);
            let free = held
                .get(&screen)
                .map(|lease| lease.expired(now))
                .unwrap_or(true);

            if free {
                let lease = ScreenLease {
                    screen,
                    holder: holder.clone(),
                    fence,
                    expires_at: now + ttl,
                };
                held.insert(screen, lease.clone());
                return Ok(lease);
            }
        }

        Err(Error::ScreenUnavailable {
            screen: None,
            held_by: None,
        })
    }

    /// Take one particular screen, whoever has it.
    ///
    /// Succeeds when the incoming fence is higher than the held one, which is
    /// how a caller recovers a screen from one that never came back.
    pub fn take(
        &self,
        screen: ScreenId,
        holder: &HolderId,
        fence: u64,
        now: SystemTime,
        ttl: Duration,
    ) -> Result<ScreenLease> {
        if screen.0 >= self.max {
            return Err(Error::ScreenUnavailable {
                screen: Some(screen),
                held_by: None,
            });
        }

        let mut held = self.lock()?;

        if let Some(existing) = held.get(&screen) {
            let contested =
                !existing.expired(now) && &existing.holder != holder && fence <= existing.fence;

            if contested {
                return Err(Error::ScreenUnavailable {
                    screen: Some(screen),
                    held_by: Some(existing.holder.clone()),
                });
            }
        }

        let lease = ScreenLease {
            screen,
            holder: holder.clone(),
            fence,
            expires_at: now + ttl,
        };
        held.insert(screen, lease.clone());
        Ok(lease)
    }

    /// Give a screen back.
    ///
    /// Succeeds only for the same holder at a fence at least as high, so a
    /// stale release cannot tear down its replacement's screen.
    pub fn release(&self, lease: &ScreenLease) -> Result<()> {
        let mut held = self.lock()?;

        match held.get(&lease.screen) {
            Some(current) if current.holder == lease.holder && lease.fence >= current.fence => {
                held.remove(&lease.screen);
                Ok(())
            }
            Some(current) => Err(Error::ScreenUnavailable {
                screen: Some(lease.screen),
                held_by: Some(current.holder.clone()),
            }),
            // Already gone. Releasing twice is not an error.
            None => Ok(()),
        }
    }

    pub fn holder_of(&self, screen: ScreenId, now: SystemTime) -> Option<HolderId> {
        self.lock()
            .ok()?
            .get(&screen)
            .filter(|lease| !lease.expired(now))
            .map(|lease| lease.holder.clone())
    }

    /// Drop every lease that has run out, so the count and the map agree.
    pub fn sweep(&self, now: SystemTime) -> usize {
        let Ok(mut held) = self.lock() else {
            return 0;
        };
        let before = held.len();
        held.retain(|_, lease| !lease.expired(now));
        before - held.len()
    }
}

/// Who is driving a screen — the owner, or a person who took it over.
///
/// Fenced by a token for the same reason a lease is fenced: ending your own
/// takeover must not end the takeover of whoever replaced you.
pub struct ControlGate {
    state: Mutex<(Control, Option<String>)>,
    /// How many takeovers this gate has seen, ever.
    ///
    /// A driver that remembers something about the screen can hold this beside
    /// it and tell that a person has driven since.
    takeovers: AtomicU64,
}

impl Default for ControlGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlGate {
    pub fn new() -> Self {
        Self {
            state: Mutex::new((Control::Owner, None)),
            takeovers: AtomicU64::new(0),
        }
    }

    /// How many takeovers have been started on this screen.
    ///
    /// Read it beside anything a driver remembers about the screen, and
    /// compare before trusting that memory.
    pub fn takeovers(&self) -> u64 {
        self.takeovers.load(Ordering::Relaxed)
    }

    pub fn control(&self) -> Control {
        self.state
            .lock()
            .map(|held| held.0.clone())
            .unwrap_or(Control::Owner)
    }

    /// Hand the input to a person.
    pub fn hand_over(&self, token: impl Into<String>, since: SystemTime) {
        self.takeovers.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut held) = self.state.lock() {
            *held = (Control::Human { since }, Some(token.into()));
        }
    }

    /// Give the input back, if this is the takeover that is actually running.
    ///
    /// Returns false when someone else holds it now — the stale caller's
    /// release is refused rather than obeyed.
    pub fn hand_back(&self, token: &str) -> bool {
        let Ok(mut held) = self.state.lock() else {
            return false;
        };

        match &held.1 {
            Some(current) if current == token => {
                *held = (Control::Owner, None);
                true
            }
            Some(_) => false,
            None => true,
        }
    }

    /// Take the input back with no token.
    ///
    /// A caller deciding the person is finished, rather than a stale release
    /// arriving late. Nothing reaches this on a timeout or a retry.
    pub fn reclaim(&self) {
        if let Ok(mut held) = self.state.lock() {
            *held = (Control::Owner, None);
        }
    }

    /// Whether the owner may still send input.
    ///
    /// Reads stay allowed while a person drives: only the input is withheld.
    pub fn may_act(&self) -> Result<()> {
        match self.control() {
            Control::Owner => Ok(()),
            Control::Human { .. } => Err(Error::denied(
                "a person is driving this screen; observe, do not act",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn holder(name: &str) -> HolderId {
        HolderId::new(name)
    }

    const TTL: Duration = Duration::from_secs(600);

    #[test]
    fn test_a_claim_takes_the_lowest_free_screen() {
        let screens = Screens::new(8);
        let first = screens.claim(&holder("a"), 1, at(0), TTL).expect("claim");
        let second = screens.claim(&holder("b"), 1, at(0), TTL).expect("claim");

        assert_eq!(first.screen, ScreenId(0));
        assert_eq!(second.screen, ScreenId(1));
    }

    #[test]
    fn test_a_returning_holder_gets_the_same_screen_back() {
        let screens = Screens::new(8);
        let first = screens.claim(&holder("a"), 1, at(0), TTL).expect("claim");
        let again = screens.claim(&holder("a"), 2, at(10), TTL).expect("claim");

        assert_eq!(
            first.screen, again.screen,
            "a different screen would leave its windows and profile unreachable"
        );
        assert_eq!(screens.in_use(at(10)), 1);
    }

    #[test]
    fn test_a_full_box_refuses_rather_than_overbooking() {
        let screens = Screens::new(2);
        screens.claim(&holder("a"), 1, at(0), TTL).expect("claim");
        screens.claim(&holder("b"), 1, at(0), TTL).expect("claim");

        let error = screens
            .claim(&holder("c"), 1, at(0), TTL)
            .expect_err("no room");
        assert!(matches!(error, Error::ScreenUnavailable { .. }));
    }

    #[test]
    fn test_an_expired_lease_frees_its_screen() {
        let screens = Screens::new(1);
        screens.claim(&holder("a"), 1, at(0), TTL).expect("claim");

        assert!(screens.claim(&holder("b"), 1, at(10), TTL).is_err());

        let after = screens
            .claim(&holder("b"), 1, at(601), TTL)
            .expect("the holder never came back");
        assert_eq!(after.screen, ScreenId(0));
    }

    #[test]
    fn test_a_higher_fence_takes_a_held_screen() {
        let screens = Screens::new(1);
        screens.claim(&holder("a"), 1, at(0), TTL).expect("claim");

        let taken = screens
            .take(ScreenId(0), &holder("b"), 2, at(1), TTL)
            .expect("a newer holder wins");
        assert_eq!(taken.holder, holder("b"));
    }

    #[test]
    fn test_an_equal_or_lower_fence_cannot_take() {
        let screens = Screens::new(1);
        screens.claim(&holder("a"), 5, at(0), TTL).expect("claim");

        assert!(
            screens
                .take(ScreenId(0), &holder("b"), 5, at(1), TTL)
                .is_err()
        );
        assert!(
            screens
                .take(ScreenId(0), &holder("b"), 4, at(1), TTL)
                .is_err()
        );
    }

    #[test]
    fn test_a_stale_release_cannot_tear_down_its_replacement() {
        let screens = Screens::new(1);
        let old = screens.claim(&holder("a"), 1, at(0), TTL).expect("claim");
        screens
            .take(ScreenId(0), &holder("b"), 2, at(1), TTL)
            .expect("take");

        let error = screens
            .release(&old)
            .expect_err("the slow holder must not win");
        assert!(matches!(
            error,
            Error::ScreenUnavailable {
                held_by: Some(_),
                ..
            }
        ));
        assert_eq!(screens.in_use(at(2)), 1, "b still has its screen");
    }

    #[test]
    fn test_the_holder_may_release_at_a_higher_fence() {
        let screens = Screens::new(1);
        let lease = screens.claim(&holder("a"), 1, at(0), TTL).expect("claim");

        let later = ScreenLease { fence: 2, ..lease };
        screens.release(&later).expect("same holder, newer fence");
        assert_eq!(screens.in_use(at(1)), 0);
    }

    #[test]
    fn test_releasing_twice_is_not_an_error() {
        let screens = Screens::new(1);
        let lease = screens.claim(&holder("a"), 1, at(0), TTL).expect("claim");

        screens.release(&lease).expect("first");
        screens
            .release(&lease)
            .expect("the caller wanted it free, and it is");
    }

    #[test]
    fn test_a_screen_beyond_the_limit_is_refused() {
        let screens = Screens::new(2);
        assert!(
            screens
                .take(ScreenId(2), &holder("a"), 1, at(0), TTL)
                .is_err()
        );
    }

    #[test]
    fn test_sweeping_drops_only_what_ran_out() {
        let screens = Screens::new(4);
        screens
            .claim(&holder("a"), 1, at(0), Duration::from_secs(10))
            .expect("claim");
        screens
            .claim(&holder("b"), 1, at(0), Duration::from_secs(900))
            .expect("claim");

        assert_eq!(screens.sweep(at(100)), 1);
        assert_eq!(screens.in_use(at(100)), 1);
    }

    #[test]
    fn test_a_takeover_is_counted_so_a_driver_can_distrust_what_it_remembers() {
        let gate = ControlGate::new();
        assert_eq!(gate.takeovers(), 0);

        gate.hand_over("token-1", at(0));
        assert_eq!(gate.takeovers(), 1);

        gate.hand_back("token-1");
        assert_eq!(
            gate.takeovers(),
            1,
            "a takeover that ended still happened: the pointer it moved is \
             somewhere the driver never put it"
        );

        gate.hand_over("token-2", at(5));
        assert_eq!(gate.takeovers(), 2);
    }

    #[test]
    fn test_the_owner_may_act_until_a_person_takes_over() {
        let gate = ControlGate::new();
        assert!(gate.may_act().is_ok());

        gate.hand_over("token-1", at(0));
        assert!(
            gate.may_act().is_err(),
            "two pointers on one cursor send the next click somewhere unintended"
        );

        assert!(gate.hand_back("token-1"));
        assert!(gate.may_act().is_ok());
    }

    #[test]
    fn test_ending_your_takeover_cannot_end_somebody_elses() {
        let gate = ControlGate::new();
        gate.hand_over("token-1", at(0));
        gate.hand_over("token-2", at(5));

        assert!(
            !gate.hand_back("token-1"),
            "token-1 is not driving any more"
        );
        assert!(gate.may_act().is_err(), "token-2 still has it");

        assert!(gate.hand_back("token-2"));
        assert!(gate.may_act().is_ok());
    }

    #[test]
    fn test_reclaiming_ends_a_takeover_whose_token_nobody_here_holds() {
        let gate = ControlGate::new();
        gate.hand_over("a token this process never saw", at(0));
        assert!(gate.may_act().is_err());

        gate.reclaim();
        assert!(
            gate.may_act().is_ok(),
            "a takeover started by a process that has since exited must be \
             endable, or the screen is stuck for as long as the box lives"
        );
    }

    #[test]
    fn test_control_records_when_the_person_took_it() {
        let gate = ControlGate::new();
        gate.hand_over("token", at(42));

        assert_eq!(gate.control(), Control::Human { since: at(42) });
    }
}
