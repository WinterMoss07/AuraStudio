//! Converting colors between profiles: float runs (tables, swatches) and
//! whole 8-bit buffers (Convert to profile, import, export), on all cores.

use std::sync::Arc;

use aurastudio_core::Rgba8;
use moxcms::{Layout, TransformF32Executor, TransformOptions};
use rayon::prelude::*;

use crate::{err, ColorError, Intent, Model, Profile};

/// Colors per piece given to a core.
const PIECE: usize = 16 * 1024;

/// From one profile to another, ready to run on many colors.
pub struct Converter {
    /// `None`: the same profile, nothing to do.
    t:        Option<Arc<TransformF32Executor>>,
    from_ch:  usize,
    to_ch:    usize,
}

impl Converter {
    pub fn new(from: &Profile, to: &Profile, intent: Intent) -> Result<Converter, ColorError> {
        Self::with(from.cms(), from.model(), to.cms(), to.model(), intent, from == to)
    }

    pub(crate) fn with(from: &moxcms::ColorProfile, fm: Model, to: &moxcms::ColorProfile, tm: Model, intent: Intent, same: bool) -> Result<Converter, ColorError> {
        let (from_ch, to_ch) = (fm.channels(), tm.channels());
        if same { return Ok(Converter { t: None, from_ch, to_ch }); }
        let options = TransformOptions { rendering_intent: intent.to_moxcms(), ..Default::default() };
        let t = from.create_transform_f32(fm.layout(), to, tm.layout(), options).map_err(|e| err("no transform between these profiles", e))?;
        Ok(Converter { t: Some(t), from_ch, to_ch })
    }

    /// Into the measuring hub (linear ACES, RGB).
    pub(crate) fn to_hub(from: &Profile, intent: Intent) -> Result<Converter, ColorError> {
        Self::with(from.cms(), from.model(), crate::lab::hub(), Model::Rgb, intent, false)
    }

    /// Out of the hub.
    pub(crate) fn from_hub(to: &Profile, intent: Intent) -> Result<Converter, ColorError> {
        Self::with(crate::lab::hub(), Model::Rgb, to.cms(), to.model(), intent, false)
    }

    pub fn is_identity(&self) -> bool { self.t.is_none() }
    pub fn channels(&self) -> (usize, usize) { (self.from_ch, self.to_ch) }

    /// `src` (colors of the source's channels, 0..1) into `dst`, pieces on
    /// all cores.
    pub fn run(&self, src: &[f32], dst: &mut [f32]) -> Result<(), ColorError> {
        let n = src.len() / self.from_ch;
        if src.len() != n * self.from_ch || dst.len() != n * self.to_ch { return Err(ColorError("buffer sizes don't match".into())); }
        let Some(t) = &self.t else { dst.copy_from_slice(src); return Ok(()); };
        src.par_chunks(PIECE * self.from_ch).zip(dst.par_chunks_mut(PIECE * self.to_ch))
            .try_for_each(|(s, d)| t.transform(s, d).map_err(|e| err("transform failed", e)))
    }

    /// One color.
    pub fn color(&self, c: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0; self.to_ch];
        if self.run(c, &mut out).is_err() { out.iter_mut().zip(c).for_each(|(o, v)| *o = *v); }
        out
    }
}

/// Straight 8-bit RGBA pixels from one RGB profile to another, alpha kept
/// (Convert to profile; import and export). The same profile: untouched.
pub fn convert_buffer(from: &Profile, to: &Profile, intent: Intent, px: &mut [Rgba8]) -> Result<(), ColorError> {
    if from == to || px.is_empty() { return Ok(()); }
    if from.model() != Model::Rgb || to.model() != Model::Rgb { return Err(ColorError("only RGB documents for now".into())); }
    aurastudio_core::zone!("color convert");
    let options = TransformOptions { rendering_intent: intent.to_moxcms(), ..Default::default() };
    let t = from.cms().create_transform_8bit(Layout::Rgba, to.cms(), Layout::Rgba, options).map_err(|e| err("no transform between these profiles", e))?;
    px.par_chunks_mut(PIECE).try_for_each(|part| {
        let src: Vec<u8> = part.iter().flatten().copied().collect();
        let mut dst = vec![0u8; src.len()];
        t.transform(&src, &mut dst).map_err(|e| err("transform failed", e))?;
        for (p, d) in part.iter_mut().zip(dst.chunks_exact(4)) { *p = [d[0], d[1], d[2], p[3]]; }
        Ok(())
    })
}
