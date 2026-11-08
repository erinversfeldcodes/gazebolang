//! FSRS-based spaced-repetition scheduling over Anki's 4-button grading

use fsrs::{FSRS, MemoryState};

const DESIRED_RETENTION: f32 = 0.9;

const RELEARNING_INTERVAL_DAYS: f32 = 10.0 / (24.0 * 60.0); // 10 minutes

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    Again,
    Hard,
    Good,
    Easy,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ReviewState {
    /// FSRS memory state. `None` only for `ReviewState::default()` — a
    /// card that's never been reviewed at all.
    pub stability: Option<f32>,
    pub difficulty: Option<f32>,
    pub interval_days: f32,
    pub repetitions: u32,
    pub lapses: u32,
}

impl ReviewState {
    fn memory_state(self) -> Option<MemoryState> {
        match (self.stability, self.difficulty) {
            (Some(stability), Some(difficulty)) => Some(MemoryState {
                stability,
                difficulty,
            }),
            _ => None,
        }
    }

    /// `elapsed_days` is actual time since the card was last reviewed (0
    /// for a card that's never been reviewed) — FSRS schedules differently
    /// for a card reviewed exactly on time versus one reviewed late, so
    /// this can't be derived from `self` alone.
    #[must_use]
    pub fn schedule(self, grade: Grade, elapsed_days: u32) -> Self {
        let fsrs = FSRS::default();
        let next_states = fsrs
            .next_states(self.memory_state(), DESIRED_RETENTION, elapsed_days)
            .expect(
                "FSRS::next_states with default parameters and a fixed, valid \
                 desired_retention does not fail",
            );

        match grade {
            Grade::Again => Self {
                stability: Some(next_states.again.memory.stability),
                difficulty: Some(next_states.again.memory.difficulty),
                interval_days: RELEARNING_INTERVAL_DAYS,
                repetitions: 0,
                lapses: if self.repetitions > 0 {
                    self.lapses + 1
                } else {
                    self.lapses
                },
            },
            Grade::Hard => self.graduate(&next_states.hard),
            Grade::Good => self.graduate(&next_states.good),
            Grade::Easy => self.graduate(&next_states.easy),
        }
    }

    fn graduate(self, state: &fsrs::ItemState) -> Self {
        Self {
            stability: Some(state.memory.stability),
            difficulty: Some(state.memory.difficulty),
            interval_days: state.interval,
            repetitions: self.repetitions + 1,
            lapses: self.lapses,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rounds a test fixture's `interval_days` (always non-negative) down
    /// to a whole-day `elapsed_days` input for the next `schedule` call.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn days(interval_days: f32) -> u32 {
        interval_days.round().max(0.0) as u32
    }

    #[test]
    fn again_resets_repetitions_and_schedules_a_short_relearning_step() {
        let state = ReviewState {
            stability: Some(10.0),
            difficulty: Some(5.0),
            interval_days: 10.0,
            repetitions: 3,
            lapses: 0,
        };
        let next = state.schedule(Grade::Again, 10);
        assert_eq!(next.repetitions, 0);
        assert_eq!(next.interval_days, RELEARNING_INTERVAL_DAYS);
        assert!(next.stability.unwrap() < state.stability.unwrap());
    }

    #[test]
    fn again_on_a_learned_card_counts_as_a_lapse() {
        let state = ReviewState {
            stability: Some(10.0),
            difficulty: Some(5.0),
            interval_days: 10.0,
            repetitions: 3,
            lapses: 1,
        };
        let next = state.schedule(Grade::Again, 10);
        assert_eq!(next.lapses, 2);
    }

    #[test]
    fn again_on_a_brand_new_card_is_not_a_lapse() {
        let next = ReviewState::default().schedule(Grade::Again, 0);
        assert_eq!(next.lapses, 0);
    }

    #[test]
    fn first_good_review_graduates_the_card_with_a_fitted_memory_state() {
        let next = ReviewState::default().schedule(Grade::Good, 0);
        assert_eq!(next.repetitions, 1);
        assert!(next.stability.is_some());
        assert!(next.difficulty.is_some());
        assert!(next.interval_days >= 1.0);
    }

    #[test]
    fn second_good_review_grows_the_interval_and_repetitions() {
        let first = ReviewState::default().schedule(Grade::Good, 0);
        let second = first.schedule(Grade::Good, days(first.interval_days));
        assert_eq!(second.repetitions, 2);
        assert!(second.interval_days > first.interval_days);
    }

    #[test]
    fn easy_grows_interval_faster_than_good() {
        let state = ReviewState {
            stability: Some(8.0),
            difficulty: Some(5.0),
            interval_days: 6.0,
            repetitions: 2,
            lapses: 0,
        };
        let good = state.schedule(Grade::Good, 6);
        let easy = state.schedule(Grade::Easy, 6);
        assert!(easy.interval_days > good.interval_days);
    }

    #[test]
    fn hard_grows_interval_slower_than_good_but_keeps_streak() {
        let state = ReviewState {
            stability: Some(8.0),
            difficulty: Some(5.0),
            interval_days: 6.0,
            repetitions: 2,
            lapses: 0,
        };
        let hard = state.schedule(Grade::Hard, 6);
        let good = state.schedule(Grade::Good, 6);
        assert_eq!(hard.repetitions, good.repetitions);
        assert!(hard.interval_days < good.interval_days);
    }

    #[test]
    fn relearned_card_graduates_like_a_first_good_review() {
        let lapsed = ReviewState {
            stability: Some(10.0),
            difficulty: Some(5.0),
            interval_days: 10.0,
            repetitions: 3,
            lapses: 0,
        }
        .schedule(Grade::Again, 10);
        let next = lapsed.schedule(Grade::Good, 0);
        assert_eq!(next.repetitions, 1);
        assert!(next.interval_days >= 1.0);
    }

    proptest::proptest! {
        #[test]
        fn stability_and_interval_stay_in_bounds(
            grades in proptest::collection::vec(0u8..4, 0..50),
        ) {
            let mut state = ReviewState::default();
            for g in grades {
                let grade = match g {
                    0 => Grade::Again,
                    1 => Grade::Hard,
                    2 => Grade::Good,
                    _ => Grade::Easy,
                };
                state = state.schedule(grade, days(state.interval_days));
                assert!(state.interval_days >= 0.0);
                assert!(state.lapses <= 50);
                if let Some(stability) = state.stability {
                    assert!(stability > 0.0);
                }
            }
        }
    }
}
