//! Pure decision logic for checkpoint matching (plan phase 04).
//!
//! No I/O happens here. The caller supplies the stored states and the digests
//! of the candidate's own prefixes, so every rule is testable with synthetic
//! values.

use std::collections::BTreeMap;

/// Whether a stored state belongs to a live checkpoint or to a superseded
/// snapshot kept after truncation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateKind {
    Current,
    Snapshot,
}

/// One stored complete-line prefix that a candidate may begin with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRef {
    pub identity: String,
    pub kind: StateKind,
    pub size: u64,
    pub lines: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The candidate begins with `matched`, so its first `lines` lines are already counted.
    Skip { lines: u64, matched: StateRef },
    /// The candidate overlaps nothing known; count all of it.
    New,
    /// The candidate cannot be classified safely. Nothing may be written.
    Abort(AbortReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortReason {
    /// More than one stored state matches the candidate.
    Ambiguous { candidates: Vec<String> },
    /// The candidate is compressed and legacy rows exist for its source, so overlap
    /// cannot be excluded.
    Unresolvable,
}

/// Classifies one candidate.
///
/// `digests` maps a byte length to the SHA-256 of the candidate's first that many
/// bytes. A state matches when its size has a digest and the digests are equal.
/// The caller computes digests only for sizes it asked for, so a state longer than
/// the candidate has no digest and cannot match.
pub fn decide(
    states: &[StateRef],
    digests: &BTreeMap<u64, String>,
    compressed: bool,
    legacy_rows_present: bool,
) -> Decision {
    let matches: Vec<&StateRef> = states
        .iter()
        .filter(|state| {
            digests
                .get(&state.size)
                .is_some_and(|digest| digest == &state.sha256)
        })
        .collect();
    match matches.as_slice() {
        [] if compressed && legacy_rows_present => Decision::Abort(AbortReason::Unresolvable),
        [] => Decision::New,
        [only] => Decision::Skip {
            lines: only.lines,
            matched: (*only).clone(),
        },
        many => Decision::Abort(AbortReason::Ambiguous {
            candidates: many.iter().map(|state| state.identity.clone()).collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(identity: &str, kind: StateKind, size: u64, sha: &str) -> StateRef {
        StateRef {
            identity: identity.into(),
            kind,
            size,
            lines: size / 10,
            sha256: sha.into(),
        }
    }

    fn digests(pairs: &[(u64, &str)]) -> BTreeMap<u64, String> {
        pairs
            .iter()
            .map(|(size, sha)| (*size, (*sha).to_owned()))
            .collect()
    }

    #[test]
    fn no_states_means_new() {
        assert_eq!(decide(&[], &digests(&[]), false, false), Decision::New);
    }

    #[test]
    fn unique_prefix_match_skips_the_state_lines() {
        let states = [state("x", StateKind::Current, 100, "aaa")];
        let decision = decide(&states, &digests(&[(100, "aaa")]), true, false);
        assert_eq!(
            decision,
            Decision::Skip {
                lines: 10,
                matched: states[0].clone()
            }
        );
    }

    #[test]
    fn full_match_is_a_skip_of_the_whole_state() {
        // A candidate equal to a stored state has the state's size and digest.
        let states = [state("snap", StateKind::Snapshot, 40, "bbb")];
        assert!(matches!(
            decide(&states, &digests(&[(40, "bbb")]), false, false),
            Decision::Skip { lines: 4, .. }
        ));
    }

    #[test]
    fn two_matching_states_are_ambiguous() {
        let states = [
            state("x", StateKind::Current, 100, "aaa"),
            state("y", StateKind::Current, 100, "aaa"),
        ];
        assert_eq!(
            decide(&states, &digests(&[(100, "aaa")]), false, false),
            Decision::Abort(AbortReason::Ambiguous {
                candidates: vec!["x".into(), "y".into()]
            })
        );
    }

    #[test]
    fn compressed_candidate_with_legacy_rows_and_no_match_is_unresolvable() {
        let states = [state("x", StateKind::Current, 100, "aaa")];
        assert_eq!(
            decide(&states, &digests(&[(100, "zzz")]), true, true),
            Decision::Abort(AbortReason::Unresolvable)
        );
    }

    #[test]
    fn plain_candidate_with_legacy_rows_and_no_match_is_new() {
        // Plain legacy rows are resolved by the caller's legacy branch, not here.
        let states = [state("x", StateKind::Current, 100, "aaa")];
        assert_eq!(
            decide(&states, &digests(&[(100, "zzz")]), false, true),
            Decision::New
        );
    }

    #[test]
    fn state_longer_than_candidate_cannot_match() {
        let states = [state("x", StateKind::Current, 100, "aaa")];
        // The candidate is only 60 bytes, so there is no digest at 100.
        assert_eq!(
            decide(&states, &digests(&[(60, "aaa")]), false, false),
            Decision::New
        );
    }

    #[test]
    fn digest_must_match_at_the_same_size() {
        let states = [state("x", StateKind::Current, 100, "aaa")];
        assert_eq!(
            decide(&states, &digests(&[(50, "aaa")]), false, false),
            Decision::New
        );
    }
}
