//! Per-setting input → value mapping (piecewise-linear, matches libmypaint).

use crate::forge::input::BrushInput;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InputMapping {
    pub input: BrushInput,
    /// `(input_value, output_offset)` knots. libmypaint requires `x` strictly
    /// ascending. Output is added to `base_value` after summing all inputs.
    pub points: Vec<(f32, f32)>,
}

impl InputMapping {
    /// The curve at `x`, as libmypaint's mypaint_mapping_calculate (first and
    /// last segments extended; a flat or vertical one gives its y0).
    pub fn eval(&self, x: f32) -> f32 {
        let p = &self.points;
        match p.len() {
            0 => 0.0,
            1 => p[0].1,
            _ => {
                let (mut x0, mut y0) = p[0];
                let (mut x1, mut y1) = p[1];
                #[allow(clippy::needless_range_loop)]
                // libmypaint's mapping_calculate walks indices; iterator rewrite would obscure
                for i in 2..p.len() {
                    if x <= x1 {
                        break;
                    }
                    x0 = x1;
                    y0 = y1;
                    x1 = p[i].0;
                    y1 = p[i].1;
                }
                if x0 == x1 || y0 == y1 {
                    y0
                } else {
                    (y1 * (x - x0) + y0 * (x1 - x)) / (x1 - x0)
                }
            }
        }
    }
}

/// How a setting's curves combine: MyPaint adds offsets (base + Σ); the
/// others (Krita) multiply factors (base × product, sum, max, min or
/// max − min; + ln(factor) for a logarithmic setting).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Combine {
    #[default]
    MyPaint,
    Multiply,
    Sum,
    Max,
    Min,
    Difference,
}

impl Combine {
    /// The factors of a setting's curves combined (Krita modes).
    pub fn factor(self, factors: impl Iterator<Item = f32>) -> f32 {
        let mut it = factors.peekable();
        let Some(&first) = it.peek() else { return 1.0 };
        match self {
            Self::MyPaint | Self::Multiply => it.product(),
            Self::Sum => it.sum(),
            Self::Max => it.fold(first, f32::max),
            Self::Min => it.fold(first, f32::min),
            Self::Difference => {
                let (lo, hi) = it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)));
                hi - lo
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingValue {
    pub base_value: f32,
    pub inputs: Vec<InputMapping>,
    pub combine: Combine,
}

impl SettingValue {
    pub const fn constant(v: f32) -> Self {
        Self {
            base_value: v,
            inputs: Vec::new(),
            combine: Combine::MyPaint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_in_range() {
        let m = InputMapping {
            input: BrushInput::Pressure,
            points: vec![(0.0, 0.0), (1.0, 1.0)],
        };
        assert!((m.eval(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn extrapolates_with_segment_slope() {
        let m = InputMapping {
            input: BrushInput::Pressure,
            points: vec![(0.0, 0.0), (1.0, 2.0)],
        };
        assert!((m.eval(2.0) - 4.0).abs() < 1e-6);
        assert!((m.eval(-1.0) - (-2.0)).abs() < 1e-6);
    }

    #[test]
    fn duplicate_x_returns_first_y() {
        // Dieterle/Posterizer's opaque_multiply: `[(0,0),(0,1),(1,1)]`.
        // libmypaint walks left-to-right and reads y0 at duplicate-x or
        // duplicate-y knots. Before this was fixed, hokusai produced NaN
        // for the x = 0 input because the first segment had Δx = 0.
        let m = InputMapping {
            input: BrushInput::Pressure,
            points: vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0)],
        };
        assert_eq!(m.eval(0.0), 0.0);
        assert_eq!(m.eval(0.5), 1.0);
        assert_eq!(m.eval(1.0), 1.0);
        // Below the first knot still uses the first segment, returning y0.
        assert_eq!(m.eval(-0.5), 0.0);
    }

    #[test]
    fn staircase_curve_steps_correctly() {
        // Dieterle/Posterizer's custom_input random curve is a staircase
        // built from duplicated x knots that step the y value at each
        // tenth. Verify a couple of step boundaries.
        let m = InputMapping {
            input: BrushInput::Random,
            points: vec![
                (0.0, -10.0),
                (0.1, -10.0),
                (0.1, -8.0),
                (0.2, -8.0),
                (0.2, -6.0),
                (0.3, -6.0),
            ],
        };
        assert_eq!(m.eval(0.05), -10.0);
        assert_eq!(m.eval(0.15), -8.0);
        assert_eq!(m.eval(0.25), -6.0);
    }
}
