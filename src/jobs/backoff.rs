/// Delay after a reservation fails; attempts starts at one, not zero.
pub(crate) fn retry_delay_secs(base_secs: u64, attempts: u32) -> u64 {
    base_secs
        .max(1)
        .saturating_mul(2_u64.saturating_pow(attempts.saturating_sub(1)))
        .min(3_600)
}

/// Stable per-job jitter spreads retries without changing across relay crashes.
pub(crate) fn retry_delay_with_jitter(base_secs: u64, attempts: u32, job_id: uuid::Uuid) -> u64 {
    let base = retry_delay_secs(base_secs, attempts);
    let bytes = job_id.as_bytes();
    let entropy = u64::from(u16::from_be_bytes([bytes[14], bytes[15]]));
    let spread = (base / 4).max(1);
    base.saturating_add(entropy % (spread + 1)).min(3_600)
}

#[cfg(test)]
mod tests {
    use super::retry_delay_secs;

    #[test]
    fn retry_delay_grows_after_each_failed_attempt() {
        for (attempt, expected) in [(1, 3), (2, 6), (3, 12), (4, 24)] {
            assert_eq!(retry_delay_secs(3, attempt), expected);
        }
    }

    #[test]
    fn retry_delay_caps_overflow_and_never_busy_loops() {
        for (base, attempts, expected) in [
            (3, 12, 3_600),
            (u64::MAX, 1, 3_600),
            (1, u32::MAX, 3_600),
            (0, 1, 1),
        ] {
            assert_eq!(retry_delay_secs(base, attempts), expected);
        }
    }

    #[test]
    fn jitter_is_bounded_stable_and_spreads_jobs() {
        let low = uuid::Uuid::from_u128(0);
        let high = uuid::Uuid::from_u128(4);
        assert_eq!(super::retry_delay_with_jitter(20, 1, low), 20);
        assert_eq!(super::retry_delay_with_jitter(20, 1, high), 24);
        assert_eq!(super::retry_delay_with_jitter(u64::MAX, 100, high), 3_600);
    }
}
