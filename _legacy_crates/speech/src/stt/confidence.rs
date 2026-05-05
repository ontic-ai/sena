//! Confidence scoring utilities for transcription quality assessment.

/// Confidence tiers for display coloring and quality assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceTier {
    High,   // >= high_threshold (default 0.80)
    Medium, // >= medium_threshold (default 0.55)
    Low,    // < medium_threshold
}

/// Classify a confidence score into a tier using provided thresholds.
///
/// # Arguments
/// - `confidence`: Score in range [0.0, 1.0]
/// - `high`: Threshold for High tier (e.g., 0.80)
/// - `medium`: Threshold for Medium tier (e.g., 0.55)
///
/// # Returns
/// The appropriate confidence tier.
pub fn confidence_tier(confidence: f32, high: f32, medium: f32) -> ConfidenceTier {
    if confidence >= high {
        ConfidenceTier::High
    } else if confidence >= medium {
        ConfidenceTier::Medium
    } else {
        ConfidenceTier::Low
    }
}

/// Convert log probability to confidence score.
///
/// Whisper models output log probabilities, which are in the range (-∞, 0].
/// This converts them to a confidence score in [0.0, 1.0].
///
/// # Arguments
/// - `log_prob`: Log probability from the model (typically negative)
///
/// # Returns
/// Confidence score clamped to [0.0, 1.0].
pub fn log_prob_to_confidence(log_prob: f32) -> f32 {
    log_prob.exp().clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_tier_classification() {
        assert_eq!(confidence_tier(0.95, 0.80, 0.55), ConfidenceTier::High);
        assert_eq!(confidence_tier(0.80, 0.80, 0.55), ConfidenceTier::High);
        assert_eq!(confidence_tier(0.70, 0.80, 0.55), ConfidenceTier::Medium);
        assert_eq!(confidence_tier(0.55, 0.80, 0.55), ConfidenceTier::Medium);
        assert_eq!(confidence_tier(0.40, 0.80, 0.55), ConfidenceTier::Low);
        assert_eq!(confidence_tier(0.0, 0.80, 0.55), ConfidenceTier::Low);
    }

    #[test]
    fn log_prob_conversion() {
        // log(1.0) = 0.0 → confidence = 1.0
        assert!((log_prob_to_confidence(0.0) - 1.0).abs() < 0.001);

        // log(0.5) ≈ -0.693 → confidence ≈ 0.5
        let log_05 = 0.5_f32.ln();
        assert!((log_prob_to_confidence(log_05) - 0.5).abs() < 0.001);

        // Very negative log prob should clamp near 0
        assert!(log_prob_to_confidence(-10.0) < 0.001);

        // Positive log prob (shouldn't happen but handle gracefully) clamps to 1.0
        assert_eq!(log_prob_to_confidence(10.0), 1.0);
    }
}
