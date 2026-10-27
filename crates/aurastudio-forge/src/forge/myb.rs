//! `.myb` reader (libmypaint v3 brush files): `{"settings": {name:
//! {"base_value": v, "inputs": {input: [[x, y], ...]}}}}`. Unknown settings
//! and inputs are skipped.

use std::collections::BTreeMap;

use crate::forge::{ForgeBrush, BrushInput, BrushSetting, InputMapping, SettingValue};
use serde::Deserialize;

/// libmypaint's defaults (`mypaint_brush_set_defaults`) for the settings
/// whose default isn't 0, set before the file's values.
fn apply_libmypaint_defaults(brush: &mut ForgeBrush) {
    use BrushSetting::*;
    const DEFAULTS: &[(BrushSetting, f32)] = &[
        (Opaque, 1.0),
        (OpaqueLinearize, 0.9),
        (Radius, 2.0),
        (Hardness, 0.8),
        (AntiAliasing, 1.0),
        (DabsPerActualRadius, 2.0),
        (Speed1Slowness, 0.04),
        (Speed2Slowness, 0.8),
        (Speed1Gamma, 4.0),
        (Speed2Gamma, 4.0),
        (OffsetBySpeedSlowness, 1.0),
        (SmudgeLength, 0.5),
        (StrokeDurationLogarithmic, 4.0),
        (EllipticalDabRatio, 1.0),
        (EllipticalDabAngle, 90.0),
        (DirectionFilter, 2.0),
        (GridmapScaleX, 1.0),
        (GridmapScaleY, 1.0),
        (PosterizeNum, 0.05),
        // libmypaint 1.6.1's compiled default (its JSON says 1 since).
        (Paint, 0.0),
    ];
    for (setting, def) in DEFAULTS {
        brush.set(*setting, SettingValue::constant(*def));
    }
    // opaque_multiply: 0 + pressure [(0, 0), (1, 1)].
    let mut sv = SettingValue::constant(0.0);
    sv.inputs.push(InputMapping { input: BrushInput::Pressure, points: vec![(0.0, 0.0), (1.0, 1.0)] });
    brush.set(OpaqueMultiply, sv);
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    settings: BTreeMap<String, RawSetting>,
}

#[derive(Deserialize)]
struct RawSetting {
    #[serde(default)]
    base_value: f32,
    #[serde(default)]
    inputs: BTreeMap<String, Vec<[f32; 2]>>,
}

/// Parse a `.myb` JSON document.
pub fn from_str(json: &str) -> Result<ForgeBrush, Error> {
    let raw: Raw = serde_json::from_str(json)?;
    let mut brush = ForgeBrush::new();
    apply_libmypaint_defaults(&mut brush);
    for (name, rs) in raw.settings {
        let Some(setting) = BrushSetting::from_cname(&name) else { continue };
        let inputs = rs.inputs.into_iter()
            .filter_map(|(iname, points)| Some(InputMapping {
                input: BrushInput::from_cname(&iname)?,
                points: points.into_iter().map(|p| (p[0], p[1])).collect(),
            }))
            .collect();
        brush.set(setting, SettingValue { base_value: rs.base_value, inputs, ..SettingValue::default() });
    }
    Ok(brush)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_settings_and_skips_the_rest() {
        let json = r#"{
            "version": 3,
            "comment": "test",
            "settings": {
                "opaque": { "base_value": 1.0, "inputs": { "pressure": [[0.0, 0.0], [1.0, 1.0]], "future_input": [[0.0, 0.0], [1.0, 1.0]] } },
                "radius_logarithmic": { "base_value": 2.5 },
                "future_setting": { "base_value": 0.42 }
            }
        }"#;
        let b = from_str(json).unwrap();
        assert_eq!(b.get(BrushSetting::Opaque).base_value, 1.0);
        assert_eq!(b.get(BrushSetting::Radius).base_value, 2.5);
        assert_eq!(b.get(BrushSetting::Opaque).inputs.len(), 1);
        assert_eq!(b.get(BrushSetting::Opaque).inputs[0].input, BrushInput::Pressure);
        // A default the file didn't set.
        assert_eq!(b.get(BrushSetting::Hardness).base_value, 0.8);
    }
}
