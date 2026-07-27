//! Ports of the Wiring-style convenience helpers from `Utilities.h`.

/// Linearly rescales `x` from the range `in_min..in_max` to
/// `out_min..out_max`. Values outside the input range are extrapolated.
pub fn map(x: f32, in_min: f32, in_max: f32, out_min: f32, out_max: f32) -> f32 {
    (x - in_min) * (out_max - out_min) / (in_max - in_min) + out_min
}

/// Clips `x` to the range `min_val..max_val`.
pub fn constrain(x: f32, min_val: f32, max_val: f32) -> f32 {
    if x < min_val {
        min_val
    } else if x > max_val {
        max_val
    } else {
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_rescales_and_extrapolates() {
        assert_eq!(map(0.5, 0.0, 1.0, 0.0, 10.0), 5.0);
        assert_eq!(map(0.0, -1.0, 1.0, 0.0, 4.0), 2.0);
        // Outside the input range: extrapolated, not clipped.
        assert_eq!(map(2.0, 0.0, 1.0, 0.0, 10.0), 20.0);
        // Inverted output range.
        assert_eq!(map(0.25, 0.0, 1.0, 1.0, 0.0), 0.75);
    }

    #[test]
    fn constrain_clips_to_the_range() {
        assert_eq!(constrain(0.5, 0.0, 1.0), 0.5);
        assert_eq!(constrain(-0.5, 0.0, 1.0), 0.0);
        assert_eq!(constrain(1.5, 0.0, 1.0), 1.0);
    }
}
