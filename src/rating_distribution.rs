//! Seeder distribution curve selected for the replacement points design.
//! Integration with ratings and token funding is tracked in
//! docs/RATING_DISTRIBUTION.md; archived receipts retain their original curve.

use anyhow::{Result, ensure};

use crate::core::MICROS_PER_POINT;

/// Smallest integer population satisfying the curve's N > 20 requirement.
pub const MIN_POPULATION: usize = 21;

/// Evaluate y(x) = 1 + 99 (s(x) - s(N)) / (s(1) - s(N)), where
/// s(x) = 1 / (1 + exp(100 (x/N - 0.05))).
///
/// `seeders` is x and `population` is N. The result is the unscaled curve
/// value, not a token payout. Endpoints are exact; floating-point rounding
/// can also produce 1.0 for interior values deep in the tail.
pub fn seeder_distribution(seeders: usize, population: usize) -> Result<f64> {
    ensure!(population >= MIN_POPULATION, "Population N must exceed 20");
    ensure!(
        seeders > 0 && seeders <= population,
        "Seeder count x must be between 1 and N"
    );
    if seeders == 1 {
        return Ok(100.0);
    }
    if seeders == population {
        return Ok(1.0);
    }

    let n = population as f64;
    let s = |x: f64| 1.0 / (1.0 + (100.0 * (x / n - 0.05)).exp());
    let s_n = 1.0 / (1.0 + 95.0_f64.exp());
    Ok((1.0 + 99.0 * (s(seeders as f64) - s_n) / (s(1.0) - s_n)).clamp(1.0, 100.0))
}

/// Each seeder earns `score * y(seeders) / 100` points, rounded down to
/// six-decimal micropoints. This is an individual reward, not a pool to split.
/// A zero-score skip earns nothing and may have no seeders.
///
/// This calculation does not settle tokens or recalculate archived receipts.
pub fn reward_micropoints(score: u8, seeders: usize, population: usize) -> Result<u64> {
    ensure!(score <= 9, "Rating must be between 0 and 9");
    ensure!(population >= MIN_POPULATION, "Population N must exceed 20");
    if score == 0 && seeders == 0 {
        return Ok(0);
    }
    let percentage = seeder_distribution(seeders, population)?;
    let amount = u64::from(score) * MICROS_PER_POINT;
    // Integer arithmetic keeps both endpoint rewards exact in payment units.
    if seeders == 1 {
        return Ok(amount);
    }
    if seeders == population {
        return Ok(amount / 100);
    }
    Ok((amount as f64 * percentage / 100.0).floor() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_exact_across_population_sizes() {
        for n in [21, 22, 100, 1_000, 1_000_000, usize::MAX] {
            assert_eq!(seeder_distribution(1, n).unwrap(), 100.0);
            assert_eq!(seeder_distribution(n, n).unwrap(), 1.0);
        }
    }

    #[test]
    fn rejects_inputs_outside_the_curve_domain() {
        for n in [0, 1, 3, 19, 20] {
            assert!(seeder_distribution(1, n).is_err());
        }
        assert!(seeder_distribution(0, 21).is_err());
        assert!(seeder_distribution(22, 21).is_err());
    }

    #[test]
    fn interior_values_match_the_selected_curve() {
        for (x, n, expected) in [
            (2, 21, 2.899_688_971_379_068),
            (5, 100, 51.406_624_124_992_34),
            (10, 100, 1.674_728_041_730_069),
            (50, 1_000, 50.868_605_862_010_75),
            (100, 1_000, 1.667_526_289_672_712),
            (50_000, 1_000_000, 50.833_561_730_960_07),
        ] {
            assert!((seeder_distribution(x, n).unwrap() - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn values_stay_bounded_and_nonincreasing() {
        for n in [21, 22, 100, 1_000] {
            let mut previous = 100.0;
            for x in 1..=n {
                let value = seeder_distribution(x, n).unwrap();
                assert!(value.is_finite() && (1.0..=100.0).contains(&value));
                assert!(value <= previous, "Curve increased at x={x}, N={n}");
                previous = value;
            }
        }
    }

    #[test]
    fn curvature_changes_at_five_percent_of_population() {
        let n = 1_000;
        let y = |x| seeder_distribution(x, n).unwrap();
        let second_difference = |x| y(x - 1) - 2.0 * y(x) + y(x + 1);
        assert!(second_difference(40) < 0.0);
        assert!(second_difference(50).abs() < 1e-12);
        assert!(second_difference(60) > 0.0);
    }

    #[test]
    fn reward_endpoints_are_the_full_rating_and_one_percent() {
        for n in [21, 100, 1_000_000] {
            for score in 1..=9 {
                assert_eq!(
                    reward_micropoints(score, 1, n).unwrap(),
                    u64::from(score) * 1_000_000
                );
                assert_eq!(
                    reward_micropoints(score, n, n).unwrap(),
                    u64::from(score) * 10_000
                );
            }
        }
    }

    #[test]
    fn rewards_apply_the_percentage_per_seeder_and_floor_micropoints() {
        assert_eq!(reward_micropoints(5, 5, 100).unwrap(), 2_570_331);
        assert_eq!(reward_micropoints(9, 10, 100).unwrap(), 150_725);
        assert_eq!(reward_micropoints(1, 2, 21).unwrap(), 28_996);
        assert_eq!(reward_micropoints(9, 50, 1_000).unwrap(), 4_578_174);
        // Individual rewards are not divided by the number of recipients.
        assert!(5 * reward_micropoints(5, 5, 100).unwrap() > 5_000_000);
    }

    #[test]
    fn skips_earn_zero_and_invalid_reward_inputs_are_rejected() {
        for seeders in [0, 1, 5, 100] {
            assert_eq!(reward_micropoints(0, seeders, 100).unwrap(), 0);
        }
        assert!(reward_micropoints(10, 1, 100).is_err());
        assert!(reward_micropoints(5, 0, 100).is_err());
        assert!(reward_micropoints(5, 101, 100).is_err());
        assert!(reward_micropoints(0, 101, 100).is_err());
        assert!(reward_micropoints(5, 1, 20).is_err());
        assert!(reward_micropoints(0, 0, 20).is_err());
    }

    #[test]
    fn positive_rewards_stay_bounded_and_nonincreasing() {
        for n in [21, 100, 1_000] {
            for score in 1..=9 {
                let maximum = u64::from(score) * 1_000_000;
                let minimum = u64::from(score) * 10_000;
                let mut previous = maximum;
                for x in 1..=n {
                    let reward = reward_micropoints(score, x, n).unwrap();
                    assert!((minimum..=maximum).contains(&reward));
                    assert!(reward <= previous);
                    previous = reward;
                }
            }
        }
    }
}
