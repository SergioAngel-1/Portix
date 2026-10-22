use std::collections::HashMap;

use crate::detect::signals::Signal;
use crate::model::FrameworkConfidence;

/// Score a set of signals using probabilistic combination.
///
/// Formula: score(fw) = 1 - Π(1 - wᵢ)
///
/// This ensures:
/// - Monotonic: more evidence always increases score
/// - Bounded: score is always in [0, 1)
/// - Independent: signals combine without saturating
pub fn score_signals(signals: &[Signal]) -> HashMap<String, f64> {
    // Accumulate the complement of the evidence in one map: a second map and
    // a second id clone per signal bought nothing.
    let mut complements: HashMap<String, f64> = HashMap::new();

    for signal in signals {
        let comp = complements
            .entry(signal.framework_id.clone())
            .or_insert(1.0);
        *comp *= 1.0 - signal.weight;
    }

    complements
        .into_iter()
        .map(|(id, comp)| (id, 1.0 - comp))
        .collect()
}

/// Convert a score to a confidence level.
pub fn score_to_confidence(score: f64) -> FrameworkConfidence {
    // Below 0.30 the caller drops the framework entirely, so there is no band
    // under Low to name.
    if score >= 0.90 {
        FrameworkConfidence::High
    } else if score >= 0.60 {
        FrameworkConfidence::Medium
    } else {
        FrameworkConfidence::Low
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_signal() {
        let signals = vec![Signal::new("nextjs", 0.85, "cmdline:next")];
        let scores = score_signals(&signals);
        assert!((scores["nextjs"] - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_two_independent_signals() {
        let signals = vec![
            Signal::new("nextjs", 0.7, "cmdline:next"),
            Signal::new("nextjs", 0.9, "dep:next"),
        ];
        let scores = score_signals(&signals);
        // 1 - (1-0.7)*(1-0.9) = 1 - 0.3*0.1 = 1 - 0.03 = 0.97
        assert!((scores["nextjs"] - 0.97).abs() < 0.001);
    }

    #[test]
    fn test_competing_frameworks() {
        let signals = vec![
            Signal::new("nextjs", 0.85, "cmdline:next"),
            Signal::new("vite", 0.85, "cmdline:vite"),
        ];
        let scores = score_signals(&signals);
        assert_eq!(scores.len(), 2);
        assert!((scores["nextjs"] - 0.85).abs() < 0.001);
        assert!((scores["vite"] - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_confidence_levels() {
        assert_eq!(score_to_confidence(0.95), FrameworkConfidence::High);
        assert_eq!(score_to_confidence(0.75), FrameworkConfidence::Medium);
        assert_eq!(score_to_confidence(0.45), FrameworkConfidence::Low);
        assert_eq!(score_to_confidence(0.20), FrameworkConfidence::Low);
    }
}
