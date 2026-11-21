//! FSRS-based spaced-repetition scheduling over Anki's 4-button grading

use fsrs::{FSRS, FSRS6_DEFAULT_PARAMETERS, MemoryState};

const DESIRED_RETENTION: f32 = 0.9;

const RELEARNING_INTERVAL_DAYS: f32 = 10.0 / (24.0 * 60.0); // 10 minutes

/// How well a card was recalled, matching Anki's four answer buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    Again,
    Hard,
    Good,
    Easy,
}

/// A button number outside Anki's 1 to 4 range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidButton(pub u8);

impl std::fmt::Display for InvalidButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid grade button {}: expected 1 to 4", self.0)
    }
}

impl std::error::Error for InvalidButton {}

impl Grade {
    /// Maps an Anki button number (1 = Again, 2 = Hard, 3 = Good, 4 = Easy)
    /// to a `Grade`.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidButton`] carrying the given number if it is not 1
    /// to 4.
    pub const fn from_button(button: u8) -> Result<Self, InvalidButton> {
        match button {
            1 => Ok(Self::Again),
            2 => Ok(Self::Hard),
            3 => Ok(Self::Good),
            4 => Ok(Self::Easy),
            other => Err(InvalidButton(other)),
        }
    }

    /// The Anki button number for this grade; the inverse of
    /// [`Grade::from_button`].
    #[must_use]
    pub const fn button(self) -> u8 {
        match self {
            Self::Again => 1,
            Self::Hard => 2,
            Self::Good => 3,
            Self::Easy => 4,
        }
    }
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
                // fsrs 7 adds a fast-stability trace; its docs say FSRS-6
                // parameters (the default, which this module uses) set it
                // equal to `stability`, and it is not persisted.
                stability_fast: stability,
            }),
            _ => None,
        }
    }

    /// `elapsed_days` is actual time since the card was last reviewed (0
    /// for a card that's never been reviewed) — FSRS schedules differently
    /// for a card reviewed exactly on time versus one reviewed late, so
    /// this can't be derived from `self` alone.
    ///
    /// # Panics
    ///
    /// Never in practice: the only failure modes `FSRS::next_states` has
    /// are invalid model parameters or an invalid `desired_retention`,
    /// both of which are fixed, known-valid constants here.
    #[must_use]
    pub fn schedule(self, grade: Grade, elapsed_days: u32) -> Self {
        // `FSRS::default()` is FSRS-7 as of fsrs 7.0.0, which moves intervals
        // and needs a persisted fast-stability trace. Stay on the FSRS-6
        // parameters this app has always scheduled with.
        let fsrs =
            FSRS::new(&FSRS6_DEFAULT_PARAMETERS).expect("the FSRS-6 default parameters are valid");
        let next_states = fsrs
            .next_states(self.memory_state(), DESIRED_RETENTION, elapsed_days)
            .expect(
                "FSRS::next_states with the FSRS-6 default parameters and a fixed, valid \
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

    #[test]
    fn buttons_one_to_four_map_to_grades() {
        assert_eq!(Grade::from_button(1), Ok(Grade::Again));
        assert_eq!(Grade::from_button(2), Ok(Grade::Hard));
        assert_eq!(Grade::from_button(3), Ok(Grade::Good));
        assert_eq!(Grade::from_button(4), Ok(Grade::Easy));
    }

    #[test]
    fn out_of_range_buttons_are_errors_naming_the_number() {
        for n in [0u8, 5, 255] {
            let err = Grade::from_button(n).unwrap_err();
            assert_eq!(err, InvalidButton(n));
            assert!(err.to_string().contains(&n.to_string()));
        }
    }

    #[test]
    fn button_round_trips_for_every_grade() {
        for grade in [Grade::Again, Grade::Hard, Grade::Good, Grade::Easy] {
            assert_eq!(Grade::from_button(grade.button()), Ok(grade));
        }
    }

    /// Scheduling recorded under `fsrs` 6.6.2 for fixed inputs, as
    /// (stability, difficulty, `interval_days`, repetitions, lapses). A new
    /// `fsrs` major that moves any of these changes what a learner is shown.
    #[test]
    fn scheduling_for_fixed_inputs_matches_the_fsrs_6_record() {
        let mid = ReviewState {
            stability: Some(8.0),
            difficulty: Some(5.0),
            interval_days: 6.0,
            repetitions: 2,
            lapses: 0,
        };
        let mut rows: Vec<String> = Vec::new();
        for grade in [Grade::Again, Grade::Hard, Grade::Good, Grade::Easy] {
            for (name, state, elapsed) in [
                ("new", ReviewState::default(), 0),
                ("mid", mid, 6),
                ("late", mid, 20),
            ] {
                let n = state.schedule(grade, elapsed);
                rows.push(format!(
                    "{grade:?} {name} {:.4} {:.4} {:.4} {} {}",
                    n.stability.unwrap(),
                    n.difficulty.unwrap(),
                    n.interval_days,
                    n.repetitions,
                    n.lapses
                ));
            }
        }
        let expected: &[&str] = &[
            "Again new 0.2120 6.4133 0.0069 0 0",
            "Again mid 1.2018 8.3418 0.0069 0 1",
            "Again late 1.3994 8.3418 0.0069 0 1",
            "Hard new 1.2931 5.1122 1.2931 1 0",
            "Hard mid 16.8950 6.6660 16.8950 3 0",
            "Hard late 27.7042 6.6660 27.7042 3 0",
            "Good new 2.3065 2.1181 2.3065 1 0",
            "Good mid 22.7904 4.9902 22.7904 3 0",
            "Good late 40.7638 4.9902 40.7638 3 0",
            "Easy new 8.2956 1.0000 8.2956 1 0",
            "Easy mid 35.7010 3.3145 35.7010 3 0",
            "Easy late 69.3634 3.3145 69.3634 3 0",
        ];
        assert_eq!(rows, expected);
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
