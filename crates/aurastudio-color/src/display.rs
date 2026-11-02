//! How a document is shown: its colors through the monitor's profile, in
//! a view mode (normal, grayscale, one channel, a soft proof), as one 3D
//! table the GPU applies to the canvas. Built from a `DisplaySetup`; the
//! app rebuilds only when `key()` changes, off the UI thread, and keeps
//! the old table until the new one is ready.

use aurastudio_core::{DisplayLut, Fnv};

use crate::convert::Converter;
use crate::lab::{delta_e, luminance, Lab};
use crate::{ColorError, Intent, Profile};

/// Grid points per axis for the plain views. 33 is the usual size for
/// display tables (DisplayCAL, OCIO): smooth with trilinear interpolation.
pub const LUT_SIZE: usize = 33;
/// For a soft proof (a printer's table bends colors more sharply).
pub const PROOF_LUT_SIZE: usize = 65;

/// One channel shown alone, as gray.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Red,
    Green,
    Blue,
    /// Lab lightness.
    L,
    /// Lab a (green ↔ red), b (blue ↔ yellow): mid gray is neutral.
    A,
    B,
}

/// A printer (or any output) simulated on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Proof {
    /// The output's profile (CMYK, gray or RGB).
    pub output:        Profile,
    pub intent:        Intent,
    /// Show the paper's white and the ink's black as they are (absolute),
    /// not stretched to the screen's.
    pub paper_white:   bool,
    /// Colors the output can't print (ΔE above 2 after the trip) painted
    /// in this monitor color.
    pub gamut_warning: Option<[u8; 3]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ViewMode {
    #[default]
    Normal,
    /// Each color at its lightness, as gray.
    Grayscale,
    Channel(Channel),
    Proof(Proof),
}

/// Everything the display table depends on.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplaySetup {
    pub document: Profile,
    pub monitor:  Profile,
    /// Document → monitor.
    pub intent:   Intent,
    pub mode:     ViewMode,
}

/// ΔE past which a proofed color counts as out of gamut.
const GAMUT_DE: f32 = 2.0;

impl DisplaySetup {
    /// An sRGB document on an sRGB monitor, normal view.
    pub fn plain() -> Self { Self { document: Profile::srgb(), monitor: Profile::srgb(), intent: Intent::default(), mode: ViewMode::Normal } }

    /// A key of everything the table depends on (rebuild when it changes).
    pub fn key(&self) -> u64 {
        let mut h = Fnv::default();
        h.u64(self.document.key()).u64(self.monitor.key()).bytes(&[self.intent.code()]);
        match &self.mode {
            ViewMode::Normal => { h.bytes(&[0]); }
            ViewMode::Grayscale => { h.bytes(&[1]); }
            ViewMode::Channel(c) => { h.bytes(&[2, *c as u8]); }
            ViewMode::Proof(p) => {
                h.bytes(&[3, p.intent.code(), p.paper_white as u8]).u64(p.output.key());
                if let Some(w) = p.gamut_warning { h.bytes(&w); }
            }
        }
        h.finish()
    }

    /// Nothing to do: the GPU can skip the table.
    pub fn is_identity(&self) -> bool { self.mode == ViewMode::Normal && self.document == self.monitor }

    /// The table.
    pub fn build(&self) -> Result<DisplayLut, ColorError> {
        aurastudio_core::zone!("color display table");
        let n = if matches!(self.mode, ViewMode::Proof(_)) { PROOF_LUT_SIZE } else { LUT_SIZE };
        if self.is_identity() { return Ok(DisplayLut::identity(n)); }
        let grid = grid(n);
        let rel = Intent::RelativeColorimetric;
        let to_monitor = Converter::new(&self.document, &self.monitor, self.intent)?;
        let mut out = vec![0f32; grid.len()];
        match &self.mode {
            ViewMode::Normal => to_monitor.run(&grid, &mut out)?,
            ViewMode::Grayscale | ViewMode::Channel(Channel::L) => {
                let hub = self.hub_of(&grid)?;
                let gray: Vec<f32> = hub.chunks_exact(3).flat_map(|c| { let y = luminance([c[0], c[1], c[2]]).max(0.0); [y; 3] }).collect();
                Converter::from_hub(&self.monitor, rel)?.run(&gray, &mut out)?;
            }
            ViewMode::Channel(ch) => {
                let doc_gray: Vec<f32> = match ch {
                    Channel::Red | Channel::Green | Channel::Blue => {
                        let k = *ch as usize;
                        grid.chunks_exact(3).flat_map(|c| [c[k]; 3]).collect()
                    }
                    _ => {
                        let hub = self.hub_of(&grid)?;
                        hub.chunks_exact(3).flat_map(|c| {
                            let lab = Lab::of_hub([c[0], c[1], c[2]]);
                            let v = if *ch == Channel::A { lab.a } else { lab.b };
                            [((v + 128.0) / 255.0).clamp(0.0, 1.0); 3]
                        }).collect()
                    }
                };
                // (The value as the document's own gray, then shown.)
                to_monitor.run(&doc_gray, &mut out)?;
            }
            ViewMode::Proof(p) => self.proof(p, &grid, &mut out)?,
        }
        let texels = out.chunks_exact(3).map(|c| [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0), 1.0]).collect();
        Ok(DisplayLut { size: n, texels })
    }

    fn hub_of(&self, grid: &[f32]) -> Result<Vec<f32>, ColorError> {
        let mut hub = vec![0f32; grid.len()];
        Converter::to_hub(&self.document, Intent::RelativeColorimetric)?.run(grid, &mut hub)?;
        Ok(hub)
    }

    /// Document → output → monitor; out-of-gamut colors marked.
    fn proof(&self, p: &Proof, grid: &[f32], out: &mut [f32]) -> Result<(), ColorError> {
        let to_out = Converter::new(&self.document, &p.output, p.intent)?;
        let ch = p.output.model().channels();
        let mut printed = vec![0f32; grid.len() / 3 * ch];
        to_out.run(grid, &mut printed)?;
        let show = if p.paper_white { Intent::AbsoluteColorimetric } else { Intent::RelativeColorimetric };
        Converter::new(&p.output, &self.monitor, show)?.run(&printed, out)?;
        if let Some(warn) = p.gamut_warning {
            let rel = Intent::RelativeColorimetric;
            let (wanted, got) = (self.hub_of(grid)?, {
                let mut back = vec![0f32; grid.len()];
                Converter::to_hub(&p.output, rel)?.run(&printed, &mut back)?;
                back
            });
            let warn = warn.map(aurastudio_core::math::from_u8);
            for ((o, w), g) in out.chunks_exact_mut(3).zip(wanted.chunks_exact(3)).zip(got.chunks_exact(3)) {
                let de = delta_e(Lab::of_hub([w[0], w[1], w[2]]), Lab::of_hub([g[0], g[1], g[2]]));
                if de > GAMUT_DE { o.copy_from_slice(&warn); }
            }
        }
        Ok(())
    }
}

/// The table's grid points, red fastest, then green, then blue.
fn grid(n: usize) -> Vec<f32> {
    let k = 1.0 / (n - 1) as f32;
    let mut v = Vec::with_capacity(n * n * n * 3);
    for b in 0..n { for g in 0..n { for r in 0..n { v.extend_from_slice(&[r as f32 * k, g as f32 * k, b as f32 * k]); } } }
    v
}
