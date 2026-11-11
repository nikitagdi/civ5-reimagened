//! Builders of the model roots: plain functions from core records to generated root structs.

pub mod game;
pub mod top;

/// `part / whole` rounded to 1/1000; 0 when `whole` is not positive.
#[must_use]
pub fn fraction(part: i32, whole: i32) -> f32 {
    if whole <= 0 {
        return 0.0;
    }
    let f = f64::from(part) / f64::from(whole);
    ((f * 1000.0).round() / 1000.0) as f32
}

#[cfg(test)]
mod tests {
    use super::fraction;

    #[test]
    fn fraction_rounds_to_thousandths() {
        assert_eq!(fraction(310, 500), 0.62);
        assert_eq!(fraction(1, 3), 0.333);
        assert_eq!(fraction(2, 3), 0.667);
        assert_eq!(fraction(5, 0), 0.0);
        assert_eq!(fraction(5, -1), 0.0);
    }
}
