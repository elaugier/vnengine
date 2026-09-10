#[cfg(test)]
mod tests {
    use crate::manager::db_from_linear;
    use kira::Decibels;

    #[test]
    fn db_from_linear_zero_is_silence() {
        assert_eq!(db_from_linear(0.0), Decibels::SILENCE);
        assert_eq!(db_from_linear(-1.0), Decibels::SILENCE);
    }

    #[test]
    fn db_from_linear_one_is_identity() {
        // 20 * log10(1.0) = 0.0 dB
        let db = db_from_linear(1.0);
        assert!((db.0 - 0.0).abs() < 0.01);
    }

    #[test]
    fn db_from_linear_half_is_minus_six() {
        // 20 * log10(0.5) ≈ -6.02 dB
        let db = db_from_linear(0.5);
        assert!((db.0 - (-6.02)).abs() < 0.1);
    }

    #[test]
    fn db_from_linear_monotonic() {
        let d1 = db_from_linear(0.1).0;
        let d2 = db_from_linear(0.5).0;
        let d3 = db_from_linear(1.0).0;
        assert!(d1 < d2);
        assert!(d2 < d3);
    }
}
