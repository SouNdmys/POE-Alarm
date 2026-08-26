//! Latency bookkeeping for the lab binaries.
//!
//! The decision this lab exists to answer lives in the tail, not the median, so
//! every summary reports p95/p99/max next to p50.

use std::time::Duration;

/// A latency sample set that reports percentiles.
#[derive(Clone, Debug, Default)]
pub struct LatencySamples {
    samples: Vec<Duration>,
}

impl LatencySamples {
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
        }
    }

    pub fn push(&mut self, sample: Duration) {
        self.samples.push(sample);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Nearest-rank percentile. `percentile` is in `0.0..=1.0`.
    #[must_use]
    pub fn percentile(&self, percentile: f64) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let clamped = percentile.clamp(0.0, 1.0);
        let rank = (clamped * (sorted.len() as f64 - 1.0)).round() as usize;
        sorted.get(rank).copied()
    }

    #[must_use]
    pub fn mean(&self) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        let total: Duration = self.samples.iter().sum();
        Some(total / self.samples.len() as u32)
    }

    #[must_use]
    pub fn min(&self) -> Option<Duration> {
        self.samples.iter().copied().min()
    }

    #[must_use]
    pub fn max(&self) -> Option<Duration> {
        self.samples.iter().copied().max()
    }

    /// Fraction of samples at or below `budget`.
    #[must_use]
    pub fn fraction_within(&self, budget: Duration) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let within = self
            .samples
            .iter()
            .filter(|sample| **sample <= budget)
            .count();
        within as f64 / self.samples.len() as f64
    }

    /// One-line summary used by both binaries.
    #[must_use]
    pub fn summary(&self, label: &str) -> String {
        let Some(mean) = self.mean() else {
            return format!("{label:<28} no samples");
        };
        format!(
            "{label:<28} n={:<5} min={:<7} p50={:<7} p90={:<7} p95={:<7} p99={:<7} max={:<7} mean={}",
            self.len(),
            format_millis(self.min().unwrap_or_default()),
            format_millis(self.percentile(0.50).unwrap_or_default()),
            format_millis(self.percentile(0.90).unwrap_or_default()),
            format_millis(self.percentile(0.95).unwrap_or_default()),
            format_millis(self.percentile(0.99).unwrap_or_default()),
            format_millis(self.max().unwrap_or_default()),
            format_millis(mean),
        )
    }

    /// Coarse histogram so a bimodal distribution is visible at a glance.
    #[must_use]
    pub fn histogram(&self, buckets_ms: &[u64]) -> String {
        if self.samples.is_empty() {
            return "  (no samples)".to_string();
        }
        let mut lines = Vec::new();
        let mut previous = 0_u64;
        for boundary in buckets_ms {
            let count = self
                .samples
                .iter()
                .filter(|sample| {
                    let millis = sample.as_millis() as u64;
                    millis >= previous && millis < *boundary
                })
                .count();
            lines.push(render_bucket(
                &format!("{previous:>4}-{boundary:<4}ms"),
                count,
                self.samples.len(),
            ));
            previous = *boundary;
        }
        let overflow = self
            .samples
            .iter()
            .filter(|sample| sample.as_millis() as u64 >= previous)
            .count();
        lines.push(render_bucket(
            &format!("{previous:>4}+     ms"),
            overflow,
            self.samples.len(),
        ));
        lines.join("\n")
    }
}

fn render_bucket(label: &str, count: usize, total: usize) -> String {
    let share = count as f64 / total as f64;
    let bar_width = (share * 40.0).round() as usize;
    let bar: String = "#".repeat(bar_width);
    format!("  {label} {count:>5} {:>5.1}% {bar}", share * 100.0)
}

/// Formats a duration as milliseconds with one decimal.
#[must_use]
pub fn format_millis(duration: Duration) -> String {
    format!("{:.1}ms", duration.as_secs_f64() * 1_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(millis: &[u64]) -> LatencySamples {
        let mut set = LatencySamples::with_capacity(millis.len());
        for value in millis {
            set.push(Duration::from_millis(*value));
        }
        set
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        // Odd sample count so the median lands on a real element.
        let set = samples(&[0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100]);
        assert_eq!(set.percentile(0.0), Some(Duration::from_millis(0)));
        assert_eq!(set.percentile(1.0), Some(Duration::from_millis(100)));
        assert_eq!(set.percentile(0.5), Some(Duration::from_millis(50)));
        assert_eq!(set.percentile(0.9), Some(Duration::from_millis(90)));
    }

    #[test]
    fn percentiles_clamp_out_of_range_inputs() {
        let set = samples(&[10, 20, 30]);
        assert_eq!(set.percentile(-1.0), Some(Duration::from_millis(10)));
        assert_eq!(set.percentile(9.0), Some(Duration::from_millis(30)));
    }

    #[test]
    fn samples_arrive_unsorted_without_disturbing_percentiles() {
        let set = samples(&[100, 0, 50]);
        assert_eq!(set.percentile(0.0), Some(Duration::from_millis(0)));
        assert_eq!(set.percentile(0.5), Some(Duration::from_millis(50)));
        assert_eq!(set.percentile(1.0), Some(Duration::from_millis(100)));
    }

    #[test]
    fn empty_sets_report_nothing() {
        let set = LatencySamples::default();
        assert!(set.is_empty());
        assert_eq!(set.percentile(0.5), None);
        assert_eq!(set.mean(), None);
        assert!(set.summary("empty").contains("no samples"));
    }

    #[test]
    fn fraction_within_counts_inclusive_budget() {
        let set = samples(&[10, 20, 30, 40]);
        assert!((set.fraction_within(Duration::from_millis(20)) - 0.5).abs() < f64::EPSILON);
        assert!((set.fraction_within(Duration::from_millis(40)) - 1.0).abs() < f64::EPSILON);
        assert!(set.fraction_within(Duration::from_millis(5)).abs() < f64::EPSILON);
    }

    #[test]
    fn histogram_accounts_for_every_sample() {
        let set = samples(&[5, 15, 25, 500]);
        let rendered = set.histogram(&[10, 20, 30]);
        assert_eq!(rendered.lines().count(), 4);
        assert!(rendered.contains("30+"));
    }
}
