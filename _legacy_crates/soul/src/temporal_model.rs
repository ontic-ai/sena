//! Temporal user behavior model: hour-of-day and day-of-week patterns.

use bus::events::soul::TemporalBehaviorPattern;
use std::collections::HashMap;
use std::time::SystemTime;

/// Temporal model that tracks event frequency by hour-of-day and day-of-week.
pub struct TemporalModel {
    /// Event counts bucketed by (hour, day_of_week, category).
    buckets: HashMap<TemporalKey, u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TemporalKey {
    hour_of_day: u8, // 0-23
    day_of_week: u8, // 0=Monday, 6=Sunday
    category: String,
}

impl TemporalModel {
    pub fn new() -> Self {
        Self {
            buckets: HashMap::new(),
        }
    }

    /// Record an event at the given timestamp with a category label.
    pub fn record_event(&mut self, timestamp: SystemTime, category: &str) {
        let (hour, day_of_week) = match extract_time_components(timestamp) {
            Some(t) => t,
            None => return, // Invalid timestamp, skip
        };

        let key = TemporalKey {
            hour_of_day: hour,
            day_of_week,
            category: category.to_string(),
        };

        *self.buckets.entry(key).or_insert(0) += 1;
    }

    /// Get the top N temporal patterns sorted by frequency (descending).
    pub fn top_patterns(&self, limit: usize) -> Vec<TemporalBehaviorPattern> {
        let mut patterns: Vec<_> = self
            .buckets
            .iter()
            .map(|(key, &frequency)| TemporalBehaviorPattern {
                hour_of_day: Some(key.hour_of_day),
                day_of_week: Some(key.day_of_week),
                behavior_category: key.category.clone(),
                frequency,
            })
            .collect();

        patterns.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        patterns.truncate(limit);
        patterns
    }
}

impl Default for TemporalModel {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract (hour_of_day, day_of_week) from SystemTime.
/// Returns None if time is before UNIX_EPOCH or on error.
fn extract_time_components(timestamp: SystemTime) -> Option<(u8, u8)> {
    use std::time::UNIX_EPOCH;

    let duration = timestamp.duration_since(UNIX_EPOCH).ok()?;
    let secs = duration.as_secs();

    // Convert to days and hours since epoch
    let days_since_epoch = secs / 86400;
    let seconds_in_day = secs % 86400;
    let hour = (seconds_in_day / 3600) as u8;

    // UNIX epoch (1970-01-01) was a Thursday (day 3 in our 0=Mon system)
    // So: (days_since_epoch + 3) % 7 gives day_of_week where 0=Mon
    let day_of_week = ((days_since_epoch + 3) % 7) as u8;

    Some((hour, day_of_week))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn temporal_model_records_events() {
        let mut model = TemporalModel::new();
        let timestamp = UNIX_EPOCH + Duration::from_secs(3600 * 15); // 15:00 on epoch day
        model.record_event(timestamp, "inference");

        let patterns = model.top_patterns(10);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].behavior_category, "inference");
        assert_eq!(patterns[0].frequency, 1);
    }

    #[test]
    fn temporal_model_buckets_by_hour_and_day() {
        let mut model = TemporalModel::new();

        // Day 0 (Thursday, Jan 1 1970), hour 10
        let t1 = UNIX_EPOCH + Duration::from_secs(3600 * 10);
        model.record_event(t1, "work");

        // Day 0, hour 10 again — should increment same bucket
        let t2 = UNIX_EPOCH + Duration::from_secs(3600 * 10 + 1800);
        model.record_event(t2, "work");

        // Day 0, hour 15 — different bucket
        let t3 = UNIX_EPOCH + Duration::from_secs(3600 * 15);
        model.record_event(t3, "work");

        let patterns = model.top_patterns(10);
        assert_eq!(patterns.len(), 2);
        assert_eq!(patterns[0].frequency, 2); // hour 10 bucket
        assert_eq!(patterns[1].frequency, 1); // hour 15 bucket
    }

    #[test]
    fn top_patterns_sorted_by_frequency() {
        let mut model = TemporalModel::new();

        let t1 = UNIX_EPOCH + Duration::from_secs(3600 * 9);
        model.record_event(t1, "inference");
        model.record_event(t1, "inference");
        model.record_event(t1, "inference");

        let t2 = UNIX_EPOCH + Duration::from_secs(3600 * 14);
        model.record_event(t2, "inference");

        let t3 = UNIX_EPOCH + Duration::from_secs(3600 * 22);
        model.record_event(t3, "inference");
        model.record_event(t3, "inference");

        let patterns = model.top_patterns(3);
        assert_eq!(patterns.len(), 3);
        assert_eq!(patterns[0].frequency, 3); // hour 9
        assert_eq!(patterns[1].frequency, 2); // hour 22
        assert_eq!(patterns[2].frequency, 1); // hour 14
    }

    #[test]
    fn top_patterns_respects_limit() {
        let mut model = TemporalModel::new();

        for hour in 0..10 {
            let t = UNIX_EPOCH + Duration::from_secs(3600 * hour);
            model.record_event(t, "activity");
        }

        let patterns = model.top_patterns(3);
        assert_eq!(patterns.len(), 3);
    }

    #[test]
    fn extract_time_components_handles_epoch() {
        let (hour, day) = extract_time_components(UNIX_EPOCH).expect("should parse epoch");
        assert_eq!(hour, 0);
        assert_eq!(day, 3); // Thursday = 3 in 0=Monday system
    }

    #[test]
    fn extract_time_components_handles_various_times() {
        // 1 day + 12 hours = Friday 12:00
        let t = UNIX_EPOCH + Duration::from_secs(86400 + 3600 * 12);
        let (hour, day) = extract_time_components(t).expect("should parse");
        assert_eq!(hour, 12);
        assert_eq!(day, 4); // Friday = 4 in 0=Monday system

        // 3 days = Sunday 00:00
        let t = UNIX_EPOCH + Duration::from_secs(86400 * 3);
        let (hour, day) = extract_time_components(t).expect("should parse");
        assert_eq!(hour, 0);
        assert_eq!(day, 6); // Sunday = 6 in 0=Monday system
    }
}
