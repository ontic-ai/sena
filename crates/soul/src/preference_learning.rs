//! Preference learning: extract user preference from engagement signals.

use bus::events::soul::EngagementSignal;

/// Preference learner that accumulates engagement signals and distills
/// user preferences once sufficient data is available.
pub struct PreferenceLearner {
    accepted_count: u32,
    ignored_count: u32,
    interrupted_count: u32,
    follow_up_count: u32,
    /// Minimum total signals before preferences are distilled.
    min_signals: u32,
}

impl PreferenceLearner {
    pub fn new() -> Self {
        Self {
            accepted_count: 0,
            ignored_count: 0,
            interrupted_count: 0,
            follow_up_count: 0,
            min_signals: 20,
        }
    }

    /// Record an engagement signal.
    pub fn record_engagement(&mut self, signal: &EngagementSignal) {
        match signal {
            EngagementSignal::Accepted => self.accepted_count += 1,
            EngagementSignal::Ignored => self.ignored_count += 1,
            EngagementSignal::Interrupted => self.interrupted_count += 1,
            EngagementSignal::FollowUpQuery => self.follow_up_count += 1,
        }
    }

    /// Harvest preference updates when sufficient data is accumulated.
    ///
    /// Returns (key, value) pairs to write to IDENTITY_SIGNALS.
    /// Clears internal counters after harvest.
    pub fn harvest_preferences(&mut self) -> Vec<(String, String)> {
        let total = self.accepted_count
            + self.ignored_count
            + self.interrupted_count
            + self.follow_up_count;

        if total < self.min_signals {
            return Vec::new();
        }

        let mut preferences = Vec::new();

        // Verbosity preference: interrupted signals user wants shorter responses
        let interrupted_ratio = (self.interrupted_count as f32) / (total as f32);
        if interrupted_ratio >= 0.5 {
            preferences.push(("preference::verbosity".to_string(), "low".to_string()));
        } else if interrupted_ratio <= 0.2 {
            preferences.push(("preference::verbosity".to_string(), "high".to_string()));
        }

        // Engagement preference: follow-up queries signal high engagement
        let follow_up_ratio = (self.follow_up_count as f32) / (total as f32);
        if follow_up_ratio >= 0.4 {
            preferences.push(("preference::engagement".to_string(), "high".to_string()));
        } else if follow_up_ratio <= 0.1 {
            preferences.push(("preference::engagement".to_string(), "low".to_string()));
        }

        // Proactive preference: ignored signals user doesn't want proactive responses
        let ignored_ratio = (self.ignored_count as f32) / (total as f32);
        if ignored_ratio >= 0.6 {
            preferences.push(("preference::proactive".to_string(), "low".to_string()));
        } else if ignored_ratio <= 0.2 {
            preferences.push(("preference::proactive".to_string(), "high".to_string()));
        }

        // Clear counters after harvest
        self.accepted_count = 0;
        self.ignored_count = 0;
        self.interrupted_count = 0;
        self.follow_up_count = 0;

        preferences
    }
}

impl Default for PreferenceLearner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learner_requires_minimum_signals() {
        let mut learner = PreferenceLearner::new();
        for _ in 0..10 {
            learner.record_engagement(&EngagementSignal::Accepted);
        }

        let prefs = learner.harvest_preferences();
        assert!(prefs.is_empty()); // Below min_signals threshold
    }

    #[test]
    fn learner_distills_low_verbosity_from_interruptions() {
        let mut learner = PreferenceLearner::new();

        // 12 interrupted, 8 accepted → 60% interrupted
        for _ in 0..12 {
            learner.record_engagement(&EngagementSignal::Interrupted);
        }
        for _ in 0..8 {
            learner.record_engagement(&EngagementSignal::Accepted);
        }

        let prefs = learner.harvest_preferences();
        assert!(prefs
            .iter()
            .any(|(k, v)| k == "preference::verbosity" && v == "low"));
    }

    #[test]
    fn learner_distills_high_engagement_from_follow_ups() {
        let mut learner = PreferenceLearner::new();

        // 10 follow-ups, 15 accepted → 40% follow-up
        for _ in 0..10 {
            learner.record_engagement(&EngagementSignal::FollowUpQuery);
        }
        for _ in 0..15 {
            learner.record_engagement(&EngagementSignal::Accepted);
        }

        let prefs = learner.harvest_preferences();
        assert!(prefs
            .iter()
            .any(|(k, v)| k == "preference::engagement" && v == "high"));
    }

    #[test]
    fn learner_distills_low_proactive_from_ignored_signals() {
        let mut learner = PreferenceLearner::new();

        // 15 ignored, 10 accepted → 60% ignored
        for _ in 0..15 {
            learner.record_engagement(&EngagementSignal::Ignored);
        }
        for _ in 0..10 {
            learner.record_engagement(&EngagementSignal::Accepted);
        }

        let prefs = learner.harvest_preferences();
        assert!(prefs
            .iter()
            .any(|(k, v)| k == "preference::proactive" && v == "low"));
    }

    #[test]
    fn harvest_clears_counters() {
        let mut learner = PreferenceLearner::new();

        for _ in 0..25 {
            learner.record_engagement(&EngagementSignal::Accepted);
        }

        let prefs1 = learner.harvest_preferences();
        assert!(!prefs1.is_empty());

        // Second harvest should return empty (counters cleared)
        let prefs2 = learner.harvest_preferences();
        assert!(prefs2.is_empty());
    }

    #[test]
    fn learner_distills_multiple_preferences() {
        let mut learner = PreferenceLearner::new();

        // 12 interrupted (low verbosity), 5 follow-ups (borderline engagement), 3 ignored
        for _ in 0..12 {
            learner.record_engagement(&EngagementSignal::Interrupted);
        }
        for _ in 0..5 {
            learner.record_engagement(&EngagementSignal::FollowUpQuery);
        }
        for _ in 0..3 {
            learner.record_engagement(&EngagementSignal::Ignored);
        }

        let prefs = learner.harvest_preferences();
        // Should get verbosity=low (60% interrupted)
        assert!(prefs
            .iter()
            .any(|(k, v)| k == "preference::verbosity" && v == "low"));
    }
}
