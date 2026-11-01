pub(super) fn maintenance_deduplication_key(timestamp_seconds: i64) -> String {
    format!("maintenance:{}", timestamp_seconds.div_euclid(60))
}

#[cfg(test)]
mod tests {
    use super::maintenance_deduplication_key;

    #[test]
    fn replicas_in_the_same_utc_minute_share_the_key() {
        assert_eq!(
            maintenance_deduplication_key(61),
            maintenance_deduplication_key(119)
        );
    }

    #[test]
    fn adjacent_minutes_get_distinct_keys() {
        assert_ne!(
            maintenance_deduplication_key(119),
            maintenance_deduplication_key(120)
        );
        assert_ne!(
            maintenance_deduplication_key(-1),
            maintenance_deduplication_key(0)
        );
    }
}
