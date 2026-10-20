//! The stroke path: libmypaint's `mypaint_brush_stroke_to`
//! (mypaint-brush.c), statement for statement, in the same order and with
//! the same f32 / f64 rounding: the random sequence and every dab match
//! libmypaint's. Per pen event: the skip window, a reset on a new stroke,
//! slow tracking, then one step per dab (settings evaluated, states
//! advanced, the dab drawn) and a last step without a dab.

use crate::forge::brush::ForgeBrush;
use crate::forge::color::{hsv_to_rgb, Hsv};
use crate::forge::evaluator::{evaluate, InputValues, SettingValues};
use crate::forge::brush::Spacing;
use crate::forge::rng::BrushRng;
use crate::forge::setting::BrushSetting;
use crate::forge::state::BrushState;
use crate::forge::surface::{Dab, TiledSurface};
use crate::forge::BrushInput;

impl ForgeBrush {
    /// One pen event, `dtime` s after the previous one. True if a dab was
    /// painted.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_to<S: TiledSurface>(
        &self,
        state: &mut BrushState,
        surface: &mut S,
        x: f32,
        y: f32,
        pressure: f32,
        xtilt: f32,
        ytilt: f32,
        dtime: f64,
    ) -> bool {
        let pressure = pressure.clamp(0.0, 1.0);
        if self.options.seeds_by_place.is_some() {
            state.event_index += 1;
            state.rng = BrushRng::new(mix32(state.event_index ^ 0x8000_0000));
        }
        let mut dtime = if dtime <= 0.0 { 0.0001 } else { dtime };
        let entry_pressure = if state.started {
            state.last_pressure
        } else {
            pressure
        };
        state.last_pressure = pressure;

        // tracking_noise's skip window: events too close are merged.
        if state.skip_distance > 0.001 {
            let dx_skip = state.skip_last_x - x;
            let dy_skip = state.skip_last_y - y;
            let dist = dx_skip.hypot(dy_skip);
            state.skip_last_x = x;
            state.skip_last_y = y;
            state.skipped_dtime += dtime;
            state.skip_distance -= dist;
            dtime = state.skipped_dtime;

            if state.skip_distance > 0.001 && dtime <= 5.0 {
                state.last_pressure = entry_pressure;
                return false;
            }

            state.skip_distance = 0.0;
            state.skip_last_x = 0.0;
            state.skip_last_y = 0.0;
            state.skipped_dtime = 0.0;
        }

        // A new stroke (brush_reset): the noise first, as libmypaint.
        if !state.started || dtime >= 5.0 {
            let base_radius_init = self
                .get(BrushSetting::Radius)
                .base_value
                .exp()
                .clamp(0.2, 1000.0);
            let noise_init = base_radius_init * self.get(BrushSetting::TrackingNoise).base_value;
            let (mut seed_x, mut seed_y) = (x, y);
            if noise_init > 0.001 {
                seed_x += state.rng.next_gauss() * noise_init;
                seed_y += state.rng.next_gauss() * noise_init;
            }
            state.skip_distance = 0.0;
            state.skip_last_x = 0.0;
            state.skip_last_y = 0.0;
            state.skipped_dtime = 0.0;
            state.random_input = state.rng.next_unit();
            state.last_event_x = seed_x;
            state.last_event_y = seed_y;
            state.actual_x = seed_x;
            state.actual_y = seed_y;
            state.actual_dab_x = seed_x;
            state.actual_dab_y = seed_y;
            state.dist_past_dab = 0.0;
            state.smudge_ra = 0.0;
            state.smudge_ga = 0.0;
            state.smudge_ba = 0.0;
            state.smudge_a = 0.0;
            state.prev_col_r = 0.0;
            state.prev_col_g = 0.0;
            state.prev_col_b = 0.0;
            state.prev_col_a = 0.0;
            state.prev_col_recentness = 0.0;
            state.norm_speed1_slow = 0.0;
            state.norm_speed2_slow = 0.0;
            state.norm_dx_slow = 0.0;
            state.norm_dy_slow = 0.0;
            state.direction_dx = 0.0;
            state.direction_dy = 0.0;
            state.direction_angle_dx = 0.0;
            state.direction_angle_dy = 0.0;
            // "As if the stroke was long finished".
            state.stroke_state = 1.0;
            state.stroke_started = false;
            state.custom_input = 0.0;
            state.flip = -1.0;
            state.actual_radius = 0.0;
            state.dabs_per_actual_radius = 0.0;
            state.dabs_per_basic_radius = 0.0;
            state.dabs_per_second = 0.0;
            state.actual_elliptical_ratio = 0.0;
            state.actual_elliptical_angle = 0.0;
            state.ascension = 0.0;
            state.declination = 0.0;
            state.declination_x = 0.0;
            state.declination_y = 0.0;
            state.started = true;
            // libmypaint ramps up from no pressure; Krita paints at once.
            if !self.options.dab_on_press {
                state.last_pressure = 0.0;
            }
            return self.options.dab_on_press && self.dab_at_rest(state, surface, seed_x, seed_y, pressure);
        }

        let stroke_threshold = self.get(BrushSetting::StrokeThreshold).base_value.max(0.0);
        const STROKE_EPS: f32 = 0.0001;

        // No tilt: upright (declination 90°).
        let (tilt_mag, tilt_declination, tilt_ascension) = if xtilt == 0.0 && ytilt == 0.0 {
            (0.0, 90.0, 0.0)
        } else {
            let m = (xtilt * xtilt + ytilt * ytilt).sqrt().min(1.0);
            (m, 90.0 - m * 60.0, (-xtilt).atan2(ytilt).to_degrees())
        };

        let mut dab_inputs = InputValues::new();
        dab_inputs.set(BrushInput::Tilt, tilt_mag);
        dab_inputs.set(BrushInput::Viewzoom, 0.0);
        dab_inputs.set(BrushInput::BarrelRotation, 0.0);
        dab_inputs.set(
            BrushInput::BrushRadius,
            self.get(BrushSetting::Radius).base_value,
        );

        let pressure_gain = self.get(BrushSetting::PressureGainLog).base_value.exp();

        let base_radius = self
            .get(BrushSetting::Radius)
            .base_value
            .exp()
            .clamp(0.2, 1000.0);

        // Slow tracking (base value only), noise before it.
        let slow = self.get(BrushSetting::SlowTracking).base_value.max(0.0);
        let approach = if slow > 1e-3 {
            1.0 - (-((100.0 * dtime) as f32) / slow).exp()
        } else {
            1.0
        };
        let (mut noisy_x, mut noisy_y) = (x, y);
        let noise_mag = base_radius * self.get(BrushSetting::TrackingNoise).base_value.max(0.0);
        if noise_mag > 0.001 {
            if self.options.seeds_by_place.is_none() {
                state.skip_distance = 0.5 * noise_mag;
            }
            state.skip_last_x = x;
            state.skip_last_y = y;
            noisy_x += state.rng.next_gauss() * noise_mag;
            noisy_y += state.rng.next_gauss() * noise_mag;
        }

        let prev_actual_x = state.actual_x;
        let prev_actual_y = state.actual_y;
        let new_actual_x = prev_actual_x + (noisy_x - prev_actual_x) * approach;
        let new_actual_y = prev_actual_y + (noisy_y - prev_actual_y) * approach;

        let smudge_setting = self.get(BrushSetting::Smudge);
        let smudge_mapping_active =
            smudge_setting.base_value != 0.0 || !smudge_setting.inputs.is_empty();

        let mut cur_x = prev_actual_x;
        let mut cur_y = prev_actual_y;
        let mut cur_ax = state.actual_dab_x;
        let mut cur_ay = state.actual_dab_y;
        let mut cur_pressure = entry_pressure;
        let mut dtime_left: f64 = dtime;
        let mut dabs_moved = state.dist_past_dab;
        let target_x = new_actual_x;
        let target_y = new_actual_y;

        let speed1 = SpeedMap::new(self.get(BrushSetting::Speed1Gamma).base_value);
        let speed2 = SpeedMap::new(self.get(BrushSetting::Speed2Gamma).base_value);
        let mut dabs_todo = count_dabs_to(
            self,
            state,
            cur_x,
            cur_y,
            target_x,
            target_y,
            dtime_left as f32,
        );
        let mut painted = false;

        // One step per dab, then one without (drawing false).
        loop {
            let drawing = dabs_moved + dabs_todo >= 1.0;
            if let (true, Some(stride)) = (drawing, self.options.seeds_by_place) {
                state.dab_index += 1;
                state.rng = BrushRng::new(mix32(state.dab_index * stride));
                state.random_input = state.rng.next_unit();
            }
            let (step_ddab, frac) = if drawing {
                let step_ddab = if dabs_moved > 0.0 {
                    1.0 - dabs_moved
                } else {
                    1.0
                };
                dabs_moved = 0.0;
                (step_ddab, step_ddab / dabs_todo)
            } else {
                (dabs_todo, 1.0)
            };

            let step_dx = frac * (target_x - cur_x);
            let step_dy = frac * (target_y - cur_y);
            let step_dpressure = frac * (pressure - cur_pressure);
            let step_dtime = (frac as f64 * dtime_left) as f32;
            let step_dtime_c = if step_dtime <= 0.0 { 0.001 } else { step_dtime };

            cur_x += step_dx;
            cur_y += step_dy;
            cur_pressure = (cur_pressure + step_dpressure).max(0.0);

            // The previous step's values, as libmypaint reads them.
            state.dabs_per_actual_radius =
                state.prev_settings.get(BrushSetting::DabsPerActualRadius);
            state.dabs_per_basic_radius = state.prev_settings.get(BrushSetting::DabsPerBasicRadius);
            state.dabs_per_second = state.prev_settings.get(BrushSetting::DabsPerSecond);

            if !state.stroke_started && cur_pressure > stroke_threshold + STROKE_EPS {
                state.stroke_started = true;
                state.stroke_state = 0.0;
            } else if state.stroke_started && cur_pressure <= stroke_threshold * 0.9 + STROKE_EPS {
                state.stroke_started = false;
            }

            let norm_dx = step_dx / step_dtime_c;
            let norm_dy = step_dy / step_dtime_c;
            let norm_speed = norm_dx.hypot(norm_dy);
            let norm_dist = {
                let ndx = step_dx / step_dtime_c / base_radius;
                let ndy = step_dy / step_dtime_c / base_radius;
                ndx.hypot(ndy) * step_dtime_c
            };

            let step_ascension = frac * smallest_angular_diff(state.ascension, tilt_ascension);
            let step_declination = frac * (tilt_declination - state.declination);
            let step_decl_x = frac * (xtilt * 60.0 - state.declination_x);
            let step_decl_y = frac * (ytilt * 60.0 - state.declination_y);
            state.ascension += step_ascension;
            state.declination += step_declination;
            state.declination_x += step_decl_x;
            state.declination_y += step_decl_y;

            dab_inputs.set(BrushInput::Pressure, cur_pressure * pressure_gain);
            dab_inputs.set(BrushInput::Stroke, state.stroke_state.min(1.0));
            dab_inputs.set(BrushInput::Speed1, speed1.input(state.norm_speed1_slow));
            dab_inputs.set(BrushInput::Speed2, speed2.input(state.norm_speed2_slow));
            dab_inputs.set(BrushInput::Random, state.random_input);

            let asc_wrapped = mod_arith_c(state.ascension + 180.0, 360.0) - 180.0;
            dab_inputs.set(BrushInput::TiltDeclination, state.declination);
            dab_inputs.set(BrushInput::TiltAscension, asc_wrapped);
            dab_inputs.set(BrushInput::TiltDeclinationX, state.declination_x);
            dab_inputs.set(BrushInput::TiltDeclinationY, state.declination_y);
            dab_inputs.set(
                BrushInput::AttackAngle,
                attack_angle(
                    state.ascension,
                    state.direction_angle_dx,
                    state.direction_angle_dy,
                ),
            );

            dab_inputs.set(BrushInput::Custom, state.custom_input);
            dab_inputs.set(
                BrushInput::Direction,
                direction_input(state.direction_dx, state.direction_dy),
            );
            dab_inputs.set(
                BrushInput::DirectionAngle,
                direction_angle(state.direction_angle_dx, state.direction_angle_dy),
            );

            const GRID_SIZE: f32 = 256.0;
            let gscale = state
                .prev_settings
                .get(BrushSetting::GridmapScale)
                .exp()
                .max(1e-3);
            let gscale_x = state.prev_settings.get(BrushSetting::GridmapScaleX);
            let gscale_y = state.prev_settings.get(BrushSetting::GridmapScaleY);
            let scaled_size = gscale * GRID_SIZE;
            let mut gx =
                (cur_ax * gscale_x).abs().rem_euclid(scaled_size) / scaled_size * GRID_SIZE;
            let mut gy =
                (cur_ay * gscale_y).abs().rem_euclid(scaled_size) / scaled_size * GRID_SIZE;
            if cur_ax < 0.0 {
                gx = GRID_SIZE - gx;
            }
            if cur_ay < 0.0 {
                gy = GRID_SIZE - gy;
            }
            dab_inputs.set(BrushInput::GridmapX, gx.clamp(0.0, GRID_SIZE));
            dab_inputs.set(BrushInput::GridmapY, gy.clamp(0.0, GRID_SIZE));

            // Settings from the states before this step; the states move
            // after, with the new settings.
            let dab_sv = evaluate(self, &dab_inputs);
            let dab_radius = dab_sv.get(BrushSetting::Radius).exp().clamp(0.2, 1000.0);
            state.actual_radius = dab_radius;

            let slow_per_dab_d = dab_sv.get(BrushSetting::SlowTrackingPerDab).max(0.0);
            let fac_ax = if slow_per_dab_d > 1e-3 {
                1.0 - (-step_ddab / slow_per_dab_d).exp()
            } else {
                1.0
            };
            cur_ax += (cur_x - cur_ax) * fac_ax;
            cur_ay += (cur_y - cur_ay) * fac_ax;

            let slow1_d = dab_sv.get(BrushSetting::Speed1Slowness).max(0.0);
            let slow2_d = dab_sv.get(BrushSetting::Speed2Slowness).max(0.0);
            let fac1 = if slow1_d > 1e-3 {
                1.0 - (-step_dtime_c / slow1_d).exp()
            } else {
                1.0
            };
            let fac2 = if slow2_d > 1e-3 {
                1.0 - (-step_dtime_c / slow2_d).exp()
            } else {
                1.0
            };
            state.norm_speed1_slow += (norm_speed - state.norm_speed1_slow) * fac1;
            state.norm_speed2_slow += (norm_speed - state.norm_speed2_slow) * fac2;

            let speed_off_slow_d = dab_sv.get(BrushSetting::OffsetBySpeedSlowness);
            let speed_off_tc = ((speed_off_slow_d * 0.01).exp() - 1.0).max(0.002);
            let fac_dx = 1.0 - (-step_dtime_c / speed_off_tc).exp();
            state.norm_dx_slow += (norm_dx - state.norm_dx_slow) * fac_dx;
            state.norm_dy_slow += (norm_dy - state.norm_dy_slow) * fac_dx;

            let dir_filter_d = dab_sv.get(BrushSetting::DirectionFilter).max(0.0);
            let dir_time_const = (dir_filter_d * 0.5).exp() - 1.0;
            let step_in_dabtime = step_dx.hypot(step_dy);
            let dir_fac = if dir_time_const > 1e-3 {
                1.0 - (-step_in_dabtime / dir_time_const).exp()
            } else {
                1.0
            };
            state.direction_angle_dx += (step_dx - state.direction_angle_dx) * dir_fac;
            state.direction_angle_dy += (step_dy - state.direction_angle_dy) * dir_fac;
            let (mut dx_for_dir, mut dy_for_dir) = (step_dx, step_dy);
            let dx_old = state.direction_dx;
            let dy_old = state.direction_dy;
            let pos_dist = (dx_old - dx_for_dir).powi(2) + (dy_old - dy_for_dir).powi(2);
            let neg_dist = (dx_old + dx_for_dir).powi(2) + (dy_old + dy_for_dir).powi(2);
            if pos_dist > neg_dist {
                dx_for_dir = -dx_for_dir;
                dy_for_dir = -dy_for_dir;
            }
            state.direction_dx += (dx_for_dir - state.direction_dx) * dir_fac;
            state.direction_dy += (dy_for_dir - state.direction_dy) * dir_fac;

            {
                let stroke_freq = (-dab_sv.get(BrushSetting::StrokeDurationLogarithmic)).exp();
                let stroke_wrap = 1.0 + dab_sv.get(BrushSetting::StrokeHoldtime).max(0.0);
                let mut stroke_advance = (state.stroke_state + norm_dist * stroke_freq).max(0.0);
                if stroke_advance >= stroke_wrap {
                    if stroke_wrap > 10.9 {
                        stroke_advance = 1.0;
                    } else {
                        stroke_advance %= stroke_wrap;
                    }
                }
                state.stroke_state = stroke_advance;
            }

            let cust_slow = dab_sv.get(BrushSetting::CustomInputSlowness).max(0.0);
            let cust_fac = if cust_slow > 1e-3 {
                1.0 - (-0.1 / cust_slow).exp()
            } else {
                1.0
            };
            let cust_target = dab_sv.get(BrushSetting::CustomInput);
            state.custom_input += (cust_target - state.custom_input) * cust_fac;

            state.actual_elliptical_ratio = dab_sv.get(BrushSetting::EllipticalDabRatio);
            state.actual_elliptical_angle =
                mod_arith_c(dab_sv.get(BrushSetting::EllipticalDabAngle) + 180.0, 180.0) - 180.0;

            state.prev_settings.values = dab_sv.values;

            if !drawing {
                break;
            }

            let mut px = cur_ax;
            let mut py = cur_ay;

            // The dab: offsets, then random offset and radius (in that
            // order: the random sequence), smudge, color.
            state.flip = -state.flip;
            let (off_x, off_y) = directional_offsets(
                &dab_sv,
                base_radius,
                state.flip,
                state.direction_angle_dx,
                state.direction_angle_dy,
                state.ascension,
            );
            px += off_x;
            py += off_y;

            // Any non-zero value draws (even clamped to 0).
            let off_random_raw = dab_sv.get(BrushSetting::OffsetByRandom);
            if off_random_raw != 0.0 {
                let off_random = off_random_raw.max(0.0);
                px += state.rng.next_gauss() * off_random * base_radius;
                py += state.rng.next_gauss() * off_random * base_radius;
            }
            let mut dab_opaque_scale = 1.0_f32;
            let mut draw_radius = dab_radius;
            let rad_random_raw = dab_sv.get(BrushSetting::RadiusByRandom);
            let rad_random = rad_random_raw.max(0.0);
            if rad_random_raw != 0.0 {
                let noise = state.rng.next_gauss() * rad_random;
                let new_log = dab_sv.get(BrushSetting::Radius) + noise;
                draw_radius = new_log.exp().clamp(0.2, 1000.0);
                let alpha_correction = (dab_radius / draw_radius).powi(2);
                if alpha_correction <= 1.0 {
                    dab_opaque_scale = alpha_correction;
                }
            }
            let off_speed = dab_sv.get(BrushSetting::OffsetBySpeed);
            if off_speed != 0.0 {
                px += state.norm_dx_slow * off_speed * 0.1;
                py += state.norm_dy_slow * off_speed * 0.1;
            }

            let smudge_amt = dab_sv.get(BrushSetting::Smudge).clamp(0.0, 1.0);
            let smudge_length = dab_sv.get(BrushSetting::SmudgeLength).clamp(0.0, 1.0);
            let mut skip_dab = false;
            // update_smudge_color.
            if smudge_length < 1.0 && smudge_mapping_active {
                let smudge_length_log = dab_sv.get(BrushSetting::SmudgeLengthLog);
                let mut update_factor = smudge_length.max(0.01);

                let recentness = state.prev_col_recentness * update_factor;
                state.prev_col_recentness = recentness;
                let threshold = (0.5 * update_factor).powf(smudge_length_log).min(1.0) + 1e-16;

                let (sr, sg, sb, sa) = if recentness < threshold {
                    if recentness == 0.0 {
                        update_factor = 0.0;
                    }
                    state.prev_col_recentness = 1.0;

                    let smudge_radius_log = dab_sv.get(BrushSetting::SmudgeRadiusLog);
                    let smudge_radius = (draw_radius * smudge_radius_log.exp()).clamp(0.2, 1000.0);
                    let paint_for_sample = dab_sv.get(BrushSetting::Paint).clamp(0.0, 1.0);
                    let sample = if paint_for_sample > 0.0 {
                        surface.get_color_pigment(px, py, smudge_radius, paint_for_sample)
                    } else {
                        surface.get_color(px, py, smudge_radius)
                    };

                    let smudge_op_lim = dab_sv.get(BrushSetting::SmudgeTransparency);
                    if (smudge_op_lim > 0.0 && sample.a < smudge_op_lim)
                        || (smudge_op_lim < 0.0 && sample.a > -smudge_op_lim)
                    {
                        skip_dab = true;
                    } else {
                        state.prev_col_r = sample.r;
                        state.prev_col_g = sample.g;
                        state.prev_col_b = sample.b;
                        state.prev_col_a = sample.a;
                    }
                    (sample.r, sample.g, sample.b, sample.a)
                } else {
                    (
                        state.prev_col_r,
                        state.prev_col_g,
                        state.prev_col_b,
                        state.prev_col_a,
                    )
                };

                if !skip_dab {
                    let fac = update_factor;
                    let paint_mode = dab_sv.get(BrushSetting::Paint).clamp(0.0, 1.0);
                    if paint_mode > 0.0 {
                        if sa > 0.01 {
                            let prev = [
                                state.smudge_ra,
                                state.smudge_ga,
                                state.smudge_ba,
                                state.smudge_a,
                            ];
                            let cur = [sr, sg, sb, sa];
                            let mixed = crate::forge::spectral::mix_colors(prev, cur, fac, paint_mode);
                            state.smudge_ra = mixed[0];
                            state.smudge_ga = mixed[1];
                            state.smudge_ba = mixed[2];
                            state.smudge_a = mixed[3];
                        } else {
                            state.smudge_a = (state.smudge_a + sa) * 0.5;
                        }
                    } else {
                        let fac_new = (1.0 - fac) * sa;
                        state.smudge_ra = state.smudge_ra * fac + sr * fac_new;
                        state.smudge_ga = state.smudge_ga * fac + sg * fac_new;
                        state.smudge_ba = state.smudge_ba * fac + sb * fac_new;
                        state.smudge_a = (state.smudge_a * fac + (1.0 - fac) * sa).clamp(0.0, 1.0);
                    }
                }
            }

            let dab = build_dab(
                self,
                &dab_sv,
                state,
                px,
                py,
                draw_radius,
                smudge_amt,
                dab_opaque_scale,
            );
            if !skip_dab && surface.draw_dab(&dab) {
                painted = true;
            }
            state.random_input = state.rng.next_unit();

            dtime_left -= step_dtime as f64;
            dabs_todo = count_dabs_to(
                self,
                state,
                cur_x,
                cur_y,
                target_x,
                target_y,
                dtime_left as f32,
            );
        }

        state.dist_past_dab = dabs_moved + dabs_todo;

        state.last_event_x = x;
        state.last_event_y = y;
        state.actual_x = new_actual_x;
        state.actual_y = new_actual_y;
        state.actual_dab_x = cur_ax;
        state.actual_dab_y = cur_ay;
        painted
    }

    /// A dab with the pen at rest (no speed, direction or tilt): where it
    /// lands and where it lifts (`ForgeOptions::dab_on_press`).
    pub(crate) fn dab_at_rest<S: TiledSurface>(&self, state: &mut BrushState, surface: &mut S, x: f32, y: f32, pressure: f32) -> bool {
        let mut inputs = InputValues::new();
        let gain = self.get(BrushSetting::PressureGainLog).base_value.exp();
        inputs.set(BrushInput::Pressure, pressure * gain);
        inputs.set(BrushInput::TiltDeclination, 90.0);
        inputs.set(BrushInput::BrushRadius, self.get(BrushSetting::Radius).base_value);
        inputs.set(BrushInput::Random, state.random_input);
        let sv = evaluate(self, &inputs);
        let radius = sv.get(BrushSetting::Radius).exp().clamp(0.2, 1000.0);
        state.actual_radius = radius;
        let dab = build_dab(self, &sv, state, x, y, radius, 0.0, 1.0);
        state.prev_settings = sv;
        surface.draw_dab(&dab)
    }

    /// Pen up: idle events at the last position until slow tracking has
    /// caught up (the end of the stroke painted). True if a dab was.
    pub fn finish_stroke<S: TiledSurface>(&self, state: &mut BrushState, surface: &mut S) -> bool {
        if !state.started {
            return false;
        }
        let mut painted = false;
        let p = state.last_pressure;
        for _ in 0..8 {
            painted |= self.stroke_to(
                state,
                surface,
                state.last_event_x,
                state.last_event_y,
                p,
                0.0,
                0.0,
                0.016,
            );
            let lag = ((state.last_event_x - state.actual_x).powi(2)
                + (state.last_event_y - state.actual_y).powi(2))
            .sqrt();
            if lag < 0.5 {
                break;
            }
        }
        painted
    }
}

/// opaque_multiply, 1 when the brush leaves it unset.
fn opaque_multiplier(brush: &ForgeBrush, sv: &SettingValues) -> f32 {
    let setting = brush.get(BrushSetting::OpaqueMultiply);
    if setting.base_value == 0.0 && setting.inputs.is_empty() {
        return 1.0;
    }
    sv.get(BrushSetting::OpaqueMultiply).clamp(0.0, 1.0)
}

/// libmypaint's prepare_and_draw_dab: color (smudge, then the color
/// dynamics), opacity (linearized), anti-aliasing, snap to pixel.
#[allow(clippy::too_many_arguments)]
fn build_dab(
    brush: &ForgeBrush,
    sv: &SettingValues,
    state: &BrushState,
    px: f32,
    py: f32,
    radius: f32,
    smudge_amt: f32,
    opaque_scale: f32,
) -> Dab {
    let base_h = brush.get(BrushSetting::ColorH).base_value;
    let base_s = brush.get(BrushSetting::ColorS).base_value;
    let base_v = brush.get(BrushSetting::ColorV).base_value;
    let base = hsv_to_rgb(Hsv {
        h: base_h,
        s: base_s,
        v: base_v,
    });

    let smudge_amt = smudge_amt.clamp(0.0, 1.0);
    let paint_mode = sv.get(BrushSetting::Paint).clamp(0.0, 1.0);
    let eraser_target_alpha = ((1.0 - smudge_amt) + smudge_amt * state.smudge_a).clamp(0.0, 1.0);
    let mixed_rgb = if smudge_amt <= 0.0 || eraser_target_alpha <= 0.0 {
        [base.r, base.g, base.b]
    } else if paint_mode > 0.0 {
        let smudge_color = [
            state.smudge_ra,
            state.smudge_ga,
            state.smudge_ba,
            state.smudge_a,
        ];
        let brush_color = [base.r, base.g, base.b, 1.0];
        let mixed = crate::forge::spectral::mix_colors(smudge_color, brush_color, smudge_amt, paint_mode);
        [
            mixed[0].clamp(0.0, 1.0),
            mixed[1].clamp(0.0, 1.0),
            mixed[2].clamp(0.0, 1.0),
        ]
    } else {
        let col_factor = 1.0 - smudge_amt;
        [
            ((smudge_amt * state.smudge_ra + col_factor * base.r) / eraser_target_alpha)
                .clamp(0.0, 1.0),
            ((smudge_amt * state.smudge_ga + col_factor * base.g) / eraser_target_alpha)
                .clamp(0.0, 1.0),
            ((smudge_amt * state.smudge_ba + col_factor * base.b) / eraser_target_alpha)
                .clamp(0.0, 1.0),
        ]
    };

    let dh = sv.get(BrushSetting::ChangeColorH);
    let dv = sv.get(BrushSetting::ChangeColorV);
    let dhsv_s = sv.get(BrushSetting::ChangeColorHsvS);
    let dl = sv.get(BrushSetting::ChangeColorL);
    let dhsl_s = sv.get(BrushSetting::ChangeColorHslS);
    let mut color_r = mixed_rgb[0];
    let mut color_g = mixed_rgb[1];
    let mut color_b = mixed_rgb[2];
    if dh != 0.0 || dhsv_s != 0.0 || dv != 0.0 {
        let mut hsv = crate::forge::color::rgb_to_hsv(color_r, color_g, color_b);
        hsv.h = (hsv.h + dh).rem_euclid(1.0);
        hsv.s = (hsv.s + hsv.s * hsv.v * dhsv_s).clamp(0.0, 1.0);
        hsv.v = (hsv.v + dv).clamp(0.0, 1.0);
        let rgb = hsv_to_rgb(hsv);
        color_r = rgb.r;
        color_g = rgb.g;
        color_b = rgb.b;
    }
    if dl != 0.0 || dhsl_s != 0.0 {
        let mut hsl = crate::forge::color::rgb_to_hsl(color_r, color_g, color_b);
        hsl.l = (hsl.l + dl).clamp(0.0, 1.0);
        let edge = (1.0 - hsl.l).abs().min(hsl.l.abs());
        hsl.s = (hsl.s + hsl.s * edge * 2.0 * dhsl_s).clamp(0.0, 1.0);
        let rgb = crate::forge::color::hsl_to_rgb(hsl);
        color_r = rgb.r;
        color_g = rgb.g;
        color_b = rgb.b;
    }

    let color = crate::forge::color::RgbaF32 {
        r: color_r,
        g: color_g,
        b: color_b,
        a: 1.0,
    };

    let opaque_raw = sv.get(BrushSetting::Opaque).max(0.0);
    let opaque_mult = opaque_multiplier(brush, sv);
    let mut opaque = (opaque_raw * opaque_mult).clamp(0.0, 1.0);

    let opaque_linearize = brush.get(BrushSetting::OpaqueLinearize).base_value.max(0.0);
    if opaque_linearize > 0.0 && opaque > 0.0 {
        let dpar = state.dabs_per_actual_radius;
        let dpbr = state.dabs_per_basic_radius;
        let mut dabs_per_pixel = (dpar + dpbr) * 2.0;
        if dabs_per_pixel < 1.0 {
            dabs_per_pixel = 1.0;
        }
        dabs_per_pixel = 1.0 + opaque_linearize * (dabs_per_pixel - 1.0);
        let beta = 1.0 - opaque;
        let beta_dab = beta.powf(1.0 / dabs_per_pixel);
        opaque = (1.0 - beta_dab).clamp(0.0, 1.0);
    }
    opaque *= opaque_scale;

    let mut hardness = sv.get(BrushSetting::Hardness).clamp(0.0, 1.0);
    let mut radius = radius;

    let aa_min = sv.get(BrushSetting::AntiAliasing).max(0.0);
    let current_fadeout = radius * (1.0 - hardness);
    if current_fadeout < aa_min {
        let optical = radius - (1.0 - hardness) * radius * 0.5;
        let hardness_new = (optical - aa_min * 0.5) / (optical + aa_min * 0.5);
        if hardness_new < 1.0 {
            radius = aa_min / (1.0 - hardness_new);
            hardness = hardness_new;
        }
    }

    let mut px = px;
    let mut py = py;
    let snap = sv.get(BrushSetting::SnapToPixel).clamp(0.0, 1.0);
    if snap > 0.0 {
        let snapped_x = px.floor() + 0.5;
        let snapped_y = py.floor() + 0.5;
        px += (snapped_x - px) * snap;
        py += (snapped_y - py) * snap;
        let mut snapped_radius = (radius * 2.0).round() * 0.5;
        if snapped_radius < 0.5 {
            snapped_radius = 0.5;
        }
        if snap > 0.9999 {
            snapped_radius -= 0.0001;
        }
        radius += (snapped_radius - radius) * snap;
    }

    let eraser = sv.get(BrushSetting::Eraser).clamp(0.0, 1.0);
    let alpha_eraser = (eraser_target_alpha * (1.0 - eraser)).clamp(0.0, 1.0);

    Dab {
        x: px,
        y: py,
        radius,
        color,
        opaque,
        hardness,
        alpha_eraser,
        aspect_ratio: state.actual_elliptical_ratio.max(1.0),
        angle: state.actual_elliptical_angle,
        lock_alpha: sv.get(BrushSetting::LockAlpha).clamp(0.0, 1.0),
        colorize: sv.get(BrushSetting::Colorize).clamp(0.0, 1.0),
        posterize: sv.get(BrushSetting::Posterize).clamp(0.0, 1.0),
        posterize_num: (sv.get(BrushSetting::PosterizeNum) * 100.0)
            .round()
            .clamp(1.0, 128.0),
        paint: sv.get(BrushSetting::Paint).clamp(0.0, 1.0),
    }
}

/// libmypaint's count_dabs_to: dabs to draw to reach the target in
/// `dt_left` s, from the states (base values where they are 0); or by
/// Forge's spacing.
fn count_dabs_to(
    brush: &ForgeBrush,
    state: &mut BrushState,
    cur_x: f32,
    cur_y: f32,
    tgt_x: f32,
    tgt_y: f32,
    dt_left: f32,
) -> f32 {
    let base_radius = brush
        .get(BrushSetting::Radius)
        .base_value
        .exp()
        .clamp(0.2, 1000.0);
    if state.actual_radius == 0.0 {
        state.actual_radius = base_radius;
    }
    if let Spacing::Fixed(px) = brush.options.spacing {
        return (tgt_x - cur_x).hypot(tgt_y - cur_y) / px.max(0.05);
    }

    let dx = tgt_x - cur_x;
    let dy = tgt_y - cur_y;
    let dist = if state.actual_elliptical_ratio > 1.0 {
        let angle_rad = radians_c(state.actual_elliptical_angle);
        let cs = angle_rad.cos();
        let sn = angle_rad.sin();
        let yyr = (dy * cs - dx * sn) * state.actual_elliptical_ratio;
        let xxr = dy * sn + dx * cs;
        (yyr * yyr + xxr * xxr).sqrt()
    } else {
        dx.hypot(dy)
    };
    if let Spacing::Auto(k) = brush.options.spacing {
        return dist / (k * (2.0 * state.actual_radius).sqrt()).max(0.5);
    }

    let dpar_state = state.dabs_per_actual_radius;
    let dpar = if dpar_state != 0.0 && !dpar_state.is_nan() {
        dpar_state
    } else {
        brush.get(BrushSetting::DabsPerActualRadius).base_value
    };
    let dpbr_state = state.dabs_per_basic_radius;
    let dpbr = if dpbr_state != 0.0 && !dpbr_state.is_nan() {
        dpbr_state
    } else {
        brush.get(BrushSetting::DabsPerBasicRadius).base_value
    };
    let dps_state = state.dabs_per_second;
    let dps = if !dps_state.is_nan() {
        dps_state
    } else {
        brush.get(BrushSetting::DabsPerSecond).base_value
    };

    dist / state.actual_radius * dpar + dist / base_radius * dpbr + dt_left * dps
}

/// A well-mixed seed from a number (never 0).
fn mix32(mut x: u32) -> u32 {
    x = (x ^ (x >> 16)).wrapping_mul(0x7feb_352d);
    x = (x ^ (x >> 15)).wrapping_mul(0x846c_a68b);
    (x ^ (x >> 16)) | 1
}

/// libmypaint's directional_offsets (no view rotation): offset_x / y and
/// the offsets along the direction, the ascension and the view, scaled by
/// base_radius × exp(offset_multiplier), at most 3240 px.
#[allow(clippy::too_many_arguments)]
fn directional_offsets(
    sv: &SettingValues,
    base_radius: f32,
    flip: f32,
    direction_angle_dx: f32,
    direction_angle_dy: f32,
    ascension_deg: f32,
) -> (f32, f32) {
    let offset_mult = sv.get(BrushSetting::OffsetMultiplier).exp();
    if !offset_mult.is_finite() {
        return (0.0, 0.0);
    }

    let mut dx = sv.get(BrushSetting::OffsetX);
    let mut dy = sv.get(BrushSetting::OffsetY);

    let offset_angle_adj = sv.get(BrushSetting::OffsetAngleAdj);
    let angle_deg =
        ((degrees_c(direction_angle_dy.atan2(direction_angle_dx)) - 90.0) as f32) % 360.0;
    let viewrotation = 0.0_f32;

    #[inline]
    fn add_polar(dx: &mut f32, dy: &mut f32, angle: f32, factor: f32) {
        let a = angle as f64;
        *dx = (*dx as f64 + a.cos() * factor as f64) as f32;
        *dy = (*dy as f64 + a.sin() * factor as f64) as f32;
    }

    let offset_angle = sv.get(BrushSetting::OffsetAngle);
    if offset_angle != 0.0 {
        let a = radians_c(angle_deg + offset_angle_adj);
        add_polar(&mut dx, &mut dy, a, offset_angle);
    }

    let offset_angle_asc = sv.get(BrushSetting::OffsetAngleAsc);
    if offset_angle_asc != 0.0 {
        let a = radians_c(ascension_deg - viewrotation + offset_angle_adj);
        add_polar(&mut dx, &mut dy, a, offset_angle_asc);
    }

    let view_offset = sv.get(BrushSetting::OffsetAngleView);
    if view_offset != 0.0 {
        let a = radians_c(viewrotation + offset_angle_adj);
        add_polar(&mut dx, &mut dy, -a, view_offset);
    }

    let offset_dir_mirror = sv.get(BrushSetting::OffsetAngle2).max(0.0);
    if offset_dir_mirror != 0.0 {
        let a = radians_c(angle_deg + offset_angle_adj * flip);
        add_polar(&mut dx, &mut dy, a, offset_dir_mirror * flip);
    }

    let offset_asc_mirror = sv.get(BrushSetting::OffsetAngle2Asc).max(0.0);
    if offset_asc_mirror != 0.0 {
        let a = radians_c(ascension_deg - viewrotation + offset_angle_adj * flip);
        add_polar(&mut dx, &mut dy, a, flip * offset_asc_mirror);
    }

    let offset_view_mirror = sv.get(BrushSetting::OffsetAngle2View).max(0.0);
    if offset_view_mirror != 0.0 {
        let a = radians_c(viewrotation + offset_angle_adj);
        add_polar(&mut dx, &mut dy, -a, flip * offset_view_mirror);
    }

    const LIM: f32 = 3240.0;
    let base_mul = base_radius * offset_mult;
    (
        (dx * base_mul).clamp(-LIM, LIM),
        (dy * base_mul).clamp(-LIM, LIM),
    )
}

/// C's DEGREES(x), in f64.
#[inline]
fn degrees_c(x: f32) -> f64 {
    x as f64 / (2.0 * std::f64::consts::PI) * 360.0
}

/// C's RADIANS(x), in f64, back to f32.
#[inline]
fn radians_c(x: f32) -> f32 {
    (x as f64 * std::f64::consts::PI / 180.0) as f32
}

/// C's mod_arith: a − N·floor(a / N), the division in f32, the rest in f64
/// (not rem_euclid's rounding).
#[inline]
fn mod_arith_c(a: f32, n: f32) -> f32 {
    (a as f64 - n as f64 * (a / n).floor() as f64) as f32
}

/// C's smallest_angular_difference(a, b), degrees.
fn smallest_angular_diff(a: f32, b: f32) -> f32 {
    let d = b - a;
    let mut r = mod_arith_c(d + 180.0, 360.0) - 180.0;
    r += if r > 180.0 {
        -360.0
    } else if r < -180.0 {
        360.0
    } else {
        0.0
    };
    r
}

/// INPUT(ATTACK_ANGLE): ascension to stroke direction + 90°, degrees.
fn attack_angle(ascension_deg: f32, dx_raw: f32, dy_raw: f32) -> f32 {
    let target = mod_arith_c((degrees_c(dy_raw.atan2(dx_raw)) + 90.0) as f32, 360.0);
    smallest_angular_diff(ascension_deg, target)
}

/// libmypaint's speed input: `ln(gamma + speed) * m + q`, through
/// (45, 0.5) with slope 0.015; the logs in f64 like libmypaint's.
struct SpeedMap { gamma: f32, m: f32, q: f32 }

impl SpeedMap {
    fn new(gamma_log: f32) -> Self {
        let gamma = gamma_log.exp();
        let m = 0.015_f32 * (45.0 + gamma);
        let c1 = (((45.0_f32 + gamma) as f64).ln()) as f32;
        Self { gamma, m, q: 0.5_f32 - m * c1 }
    }

    fn input(&self, speed_norm: f32) -> f32 {
        (((self.gamma + speed_norm) as f64).ln() * self.m as f64 + self.q as f64) as f32
    }
}

/// INPUT(DIRECTION): 0..180°.
fn direction_input(dx: f32, dy: f32) -> f32 {
    mod_arith_c((degrees_c(dy.atan2(dx)) + 180.0) as f32, 180.0)
}

/// INPUT(DIRECTION_ANGLE): 0..360°.
fn direction_angle(dx: f32, dy: f32) -> f32 {
    ((degrees_c(dy.atan2(dx)) + 360.0) as f32) % 360.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::mapping::SettingValue;

    struct CountingSurface {
        count: u32,
    }
    impl TiledSurface for CountingSurface {
        fn tile_request_start(&mut self, _tx: i32, _ty: i32) -> &mut crate::forge::tile::TilePixels {
            unreachable!()
        }
        fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}
        fn draw_dab(&mut self, _dab: &Dab) -> bool {
            self.count += 1;
            true
        }
    }

    fn make_brush(radius_log: f32, dabs_per_actual_radius: f32) -> ForgeBrush {
        let mut b = ForgeBrush::new();
        b.set(BrushSetting::Radius, SettingValue::constant(radius_log));
        b.set(
            BrushSetting::DabsPerActualRadius,
            SettingValue::constant(dabs_per_actual_radius),
        );
        b.set(BrushSetting::Opaque, SettingValue::constant(1.0));
        b.set(BrushSetting::Hardness, SettingValue::constant(0.5));
        b
    }

    #[test]
    fn no_movement_no_dabs() {
        let brush = make_brush(1.0, 2.0);
        let mut state = BrushState::default();
        let mut surf = CountingSurface { count: 0 };
        let painted = brush.stroke_to(&mut state, &mut surf, 0.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        assert!(!painted);
        assert_eq!(surf.count, 0);
    }

    #[test]
    fn moves_emit_proportional_dabs() {
        // 20 px / exp(1) ≈ 20 / 2.718 = 7.36 radii of travel. With DPAR=2
        // that's ~14.7 dabs per `count_dabs_to`. libmypaint's per-iteration
        // re-evaluation lands the integer count somewhere in this band.
        let brush = make_brush(1.0, 2.0);
        let mut state = BrushState::default();
        let mut surf = CountingSurface { count: 0 };
        brush.stroke_to(&mut state, &mut surf, 0.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        brush.stroke_to(&mut state, &mut surf, 20.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        assert!(
            (12..=16).contains(&surf.count),
            "expected ~14 dabs, got {}",
            surf.count
        );
    }

    #[test]
    fn slow_tracking_smooths_position() {
        // High slow_tracking → fewer pixels covered → fewer dabs.
        let a = make_brush(1.0, 2.0);
        let mut b = make_brush(1.0, 2.0);
        b.set(BrushSetting::SlowTracking, SettingValue::constant(0.9));

        let mut sa = BrushState::default();
        let mut sb = BrushState::default();
        let mut surf_a = CountingSurface { count: 0 };
        let mut surf_b = CountingSurface { count: 0 };

        a.stroke_to(&mut sa, &mut surf_a, 0.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        a.stroke_to(&mut sa, &mut surf_a, 20.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        b.stroke_to(&mut sb, &mut surf_b, 0.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        b.stroke_to(&mut sb, &mut surf_b, 20.0, 0.0, 1.0, 0.0, 0.0, 0.01);

        assert!(
            surf_b.count < surf_a.count,
            "slow_tracking should suppress dab count: {} >= {}",
            surf_b.count,
            surf_a.count
        );
    }

    #[test]
    fn stroke_threshold_drives_stroke_state_reset() {
        // libmypaint's stroke_threshold does *not* suppress dabs. It only
        // gates `STATE.STROKE_STARTED`: when pressure rises above the
        // threshold the stroke restarts (`stroke_state` → 0); when it falls
        // back below `threshold * 0.9` the started flag clears so the next
        // rise resets again.
        let mut brush = make_brush(1.0, 2.0);
        brush.set(BrushSetting::StrokeThreshold, SettingValue::constant(0.5));
        let mut state = BrushState::default();
        let mut surf = CountingSurface { count: 0 };
        // Pressure below threshold: started stays false, but dabs still land.
        brush.stroke_to(&mut state, &mut surf, 0.0, 0.0, 0.3, 0.0, 0.0, 0.01);
        brush.stroke_to(&mut state, &mut surf, 20.0, 0.0, 0.3, 0.0, 0.0, 0.01);
        assert!(
            surf.count > 0,
            "stroke_threshold no longer gates dab emission"
        );
        assert!(
            !state.stroke_started,
            "0.3 < threshold 0.5, started stays off"
        );

        // Above threshold (after a seed pass): started flips on and
        // stroke_state restarts at 0.
        let mut s2 = BrushState::default();
        let mut surf2 = CountingSurface { count: 0 };
        // First call always goes through the seed branch.
        brush.stroke_to(&mut s2, &mut surf2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.01);
        s2.stroke_state = 0.7;
        brush.stroke_to(&mut s2, &mut surf2, 1.0, 0.0, 0.8, 0.0, 0.0, 0.01);
        assert!(
            s2.stroke_started,
            "pressure above threshold sets started=true"
        );
        // The rising edge resets stroke_state to 0 at the step where the
        // interpolated pressure crosses the threshold; the remaining steps
        // of the event then advance it again by `norm_dist * exp(-duration)`
        // (libmypaint does the same — the value never *ends* at exactly 0).
        // It must have restarted well below the pre-seeded 0.7, though.
        assert!(
            s2.stroke_state < 0.7,
            "rising-edge reset restarts stroke_state (got {})",
            s2.stroke_state
        );
    }

    #[test]
    fn tracking_noise_shifts_dab_positions() {
        // Same input + seed should be deterministic. Two states with the same
        // seed differ in the dab positions iff tracking_noise injects gauss.
        let noise_brush = {
            let mut b = make_brush(1.0, 2.0);
            b.set(BrushSetting::TrackingNoise, SettingValue::constant(0.5));
            b
        };
        let plain = make_brush(1.0, 2.0);

        struct CaptureSurface {
            xs: Vec<f32>,
        }
        impl TiledSurface for CaptureSurface {
            fn tile_request_start(&mut self, _: i32, _: i32) -> &mut crate::forge::tile::TilePixels {
                unreachable!()
            }
            fn tile_request_end(&mut self, _: i32, _: i32) {}
            fn draw_dab(&mut self, d: &Dab) -> bool {
                self.xs.push(d.x);
                true
            }
        }

        let mut sa = BrushState::default();
        let mut sb = BrushState::default();
        let mut ca = CaptureSurface { xs: vec![] };
        let mut cb = CaptureSurface { xs: vec![] };

        plain.stroke_to(&mut sa, &mut ca, 0.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        plain.stroke_to(&mut sa, &mut ca, 20.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        noise_brush.stroke_to(&mut sb, &mut cb, 0.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        noise_brush.stroke_to(&mut sb, &mut cb, 20.0, 0.0, 1.0, 0.0, 0.0, 0.01);

        // Noise perturbs the segment length so dab counts can differ by one.
        // Compare overlapping prefixes — any difference proves noise applied.
        let any_differ = ca
            .xs
            .iter()
            .zip(cb.xs.iter())
            .any(|(a, b)| (a - b).abs() > 1e-3);
        assert!(
            any_differ || ca.xs.len() != cb.xs.len(),
            "tracking_noise should perturb the dab stream"
        );
    }

    #[test]
    fn speed_slowness_smooths_speed_input() {
        // High slowness → speed1_slow stays near 0 even after rapid event.
        let mut b = make_brush(1.0, 2.0);
        b.set(BrushSetting::Speed1Slowness, SettingValue::constant(0.99));
        let mut state = BrushState::default();
        let mut surf = CountingSurface { count: 0 };
        b.stroke_to(&mut state, &mut surf, 0.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        b.stroke_to(&mut state, &mut surf, 200.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        let smoothed = state.norm_speed1_slow;

        let b2 = make_brush(1.0, 2.0); // slowness = 0 (default)
        let mut state2 = BrushState::default();
        let mut surf2 = CountingSurface { count: 0 };
        b2.stroke_to(&mut state2, &mut surf2, 0.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        b2.stroke_to(&mut state2, &mut surf2, 200.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        let raw = state2.norm_speed1_slow;

        assert!(
            smoothed < raw,
            "slowness should suppress speed1_slow ({smoothed} >= {raw})"
        );
    }

    #[test]
    fn tilt_declination_follows_libmypaint_convention() {
        // libmypaint: `tilt_declination = 90` when the pen is straight up,
        // dropping to ~30 at the steepest tilt (`90 - hypot(x, y) * 60`).
        // With a curve mapping declination 0 → 0 and 90 → 1, the upright
        // pose feeds the *larger* contribution to the radius curve, so the
        // tilted stroke should end up *smaller* than the upright one.
        let mut tilt_brush = make_brush(1.0, 2.0);
        tilt_brush.set(
            BrushSetting::Radius,
            SettingValue {
                base_value: 1.0,
                inputs: vec![crate::forge::mapping::InputMapping {
                    input: BrushInput::TiltDeclination,
                    points: vec![(0.0, 0.0), (90.0, 1.0)],
                }],
                ..Default::default()
            },
        );
        let mut s1 = BrushState::default();
        let mut surf1 = CountingSurface { count: 0 };
        tilt_brush.stroke_to(&mut s1, &mut surf1, 0.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        tilt_brush.stroke_to(&mut s1, &mut surf1, 10.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        let r_upright = s1.actual_radius;

        let mut s2 = BrushState::default();
        let mut surf2 = CountingSurface { count: 0 };
        tilt_brush.stroke_to(&mut s2, &mut surf2, 0.0, 0.0, 1.0, 1.0, 0.0, 0.01);
        tilt_brush.stroke_to(&mut s2, &mut surf2, 10.0, 0.0, 1.0, 1.0, 0.0, 0.01);
        let r_tilted = s2.actual_radius;

        assert!(
            r_upright > r_tilted,
            "upright pen has higher declination → bigger radius via curve: {r_upright} <= {r_tilted}"
        );
    }

    #[test]
    fn change_color_h_drifts_hue() {
        // A saturated base color (gray has no hue), its hue turned per dab.
        struct Colors(Vec<f32>);
        impl TiledSurface for Colors {
            fn tile_request_start(&mut self, _: i32, _: i32) -> &mut crate::forge::tile::TilePixels { unreachable!() }
            fn tile_request_end(&mut self, _: i32, _: i32) {}
            fn draw_dab(&mut self, d: &Dab) -> bool {
                self.0.push(crate::forge::color::rgb_to_hsv(d.color.r, d.color.g, d.color.b).h);
                true
            }
        }
        let mut brush = make_brush(1.0, 2.0);
        brush.set(BrushSetting::ColorH, SettingValue::constant(0.0));
        brush.set(BrushSetting::ColorS, SettingValue::constant(1.0));
        brush.set(BrushSetting::ColorV, SettingValue::constant(1.0));
        brush.set(BrushSetting::ChangeColorH, SettingValue::constant(0.5));
        let mut state = BrushState::default();
        let mut surf = Colors(Vec::new());
        brush.stroke_to(&mut state, &mut surf, 0.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        brush.stroke_to(&mut state, &mut surf, 20.0, 0.0, 1.0, 0.0, 0.0, 0.01);
        assert!(surf.0.iter().all(|h| (h - 0.5).abs() < 0.01), "hue turned by 0.5: {:?}", surf.0);
    }
}
