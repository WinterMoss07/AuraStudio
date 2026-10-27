//! A stroke's running state: libmypaint's `states[]` and the brush's own
//! fields (smoothed position and speeds, smudge bucket, random numbers).

use crate::forge::evaluator::SettingValues;
use crate::forge::rng::BrushRng;

#[derive(Debug, Clone)]
pub struct BrushState {
    /// Slow-tracked cursor (libmypaint's STATE.X / Y).
    pub actual_x: f32,
    pub actual_y: f32,
    /// Dab center, lagging behind it by `slow_tracking_per_dab`
    /// (STATE.ACTUAL_X / Y).
    pub actual_dab_x: f32,
    pub actual_dab_y: f32,
    pub last_event_x: f32,
    pub last_event_y: f32,
    pub actual_radius: f32,

    pub norm_dx_slow: f32,
    pub norm_dy_slow: f32,
    pub norm_speed1_slow: f32,
    pub norm_speed2_slow: f32,
    /// Smoothed direction, 180°-symmetric (DIRECTION_DX / DY).
    pub direction_dx: f32,
    pub direction_dy: f32,
    /// Smoothed direction, full 360° (DIRECTION_ANGLE_DX / DY).
    pub direction_angle_dx: f32,
    pub direction_angle_dy: f32,
    pub custom_input: f32,
    /// ±1, toggled per dab (offset_angle_2*).
    pub flip: f32,
    /// tracking_noise's skip window.
    pub skip_distance: f32,
    pub skip_last_x: f32,
    pub skip_last_y: f32,
    pub skipped_dtime: f64,
    /// Smoothed tilt, degrees.
    pub ascension: f32,
    pub declination: f32,
    pub declination_x: f32,
    pub declination_y: f32,

    pub stroke_state: f32,
    pub stroke_started: bool,

    /// The previous step's setting values (libmypaint reads some one step
    /// late).
    pub dabs_per_actual_radius: f32,
    pub dabs_per_basic_radius: f32,
    pub dabs_per_second: f32,
    pub actual_elliptical_ratio: f32,
    pub actual_elliptical_angle: f32,
    pub prev_settings: SettingValues,

    /// Fraction of a dab carried to the next event.
    pub dist_past_dab: f32,

    /// Smudge bucket, premultiplied.
    pub smudge_ra: f32,
    pub smudge_ga: f32,
    pub smudge_ba: f32,
    pub smudge_a: f32,
    /// Last canvas sample (smudge_length's cache).
    pub prev_col_r: f32,
    pub prev_col_g: f32,
    pub prev_col_b: f32,
    pub prev_col_a: f32,
    pub prev_col_recentness: f32,

    pub rng: BrushRng,
    /// Seeds by place (stroke previews): dab and event numbers.
    pub dab_index: u32,
    pub event_index: u32,

    pub last_pressure: f32,
    pub random_input: f32,
    pub started: bool,
}

impl Default for BrushState {
    fn default() -> Self {
        Self {
            actual_x: 0.0, actual_y: 0.0, actual_dab_x: 0.0, actual_dab_y: 0.0,
            last_event_x: 0.0, last_event_y: 0.0, actual_radius: 0.0,
            norm_dx_slow: 0.0, norm_dy_slow: 0.0, norm_speed1_slow: 0.0, norm_speed2_slow: 0.0,
            direction_dx: 0.0, direction_dy: 0.0, direction_angle_dx: 0.0, direction_angle_dy: 0.0,
            custom_input: 0.0, flip: -1.0,
            skip_distance: 0.0, skip_last_x: 0.0, skip_last_y: 0.0, skipped_dtime: 0.0,
            ascension: 0.0, declination: 0.0, declination_x: 0.0, declination_y: 0.0,
            stroke_state: 0.0, stroke_started: false,
            dabs_per_actual_radius: 0.0, dabs_per_basic_radius: 0.0, dabs_per_second: 0.0,
            actual_elliptical_ratio: 0.0, actual_elliptical_angle: 0.0,
            prev_settings: SettingValues::new(),
            dist_past_dab: 0.0,
            smudge_ra: 0.0, smudge_ga: 0.0, smudge_ba: 0.0, smudge_a: 0.0,
            prev_col_r: 0.0, prev_col_g: 0.0, prev_col_b: 0.0, prev_col_a: 0.0, prev_col_recentness: 0.0,
            // libmypaint seeds its generator with 1000.
            rng: BrushRng::new(1000),
            dab_index: 0, event_index: 0,
            last_pressure: 0.0, random_input: 0.0, started: false,
        }
    }
}
