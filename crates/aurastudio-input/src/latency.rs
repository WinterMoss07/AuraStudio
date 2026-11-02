//! End-to-end latency, measured: each sample keeps its device time
//! through the paint thread; each presented frame says which newest sample
//! it shows and when. Per stroke: pen → screen, median and worst, split by
//! stage. The device side (tablet, USB, screen) is out of reach and the
//! same in every app.

use aurastudio_core::Micros;

/// Times of one sample on its way (all on the app's clock).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trace {
    /// When the device sampled it.
    pub sampled:   Micros,
    /// When `input` handed it over.
    pub received:  Micros,
    /// When the paint thread had painted and merged it.
    pub painted:   Micros,
    /// When the frame showing it was submitted.
    pub submitted: Micros,
    /// When that frame was presented.
    pub presented: Micros,
}

/// Microseconds per stage, median and worst, over a stroke.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stages {
    pub samples: usize,
    pub input:   (u64, u64),
    pub paint:   (u64, u64),
    pub wait:    (u64, u64),
    pub present: (u64, u64),
    pub total:   (u64, u64),
}

#[derive(Default)]
pub struct Latency { traces: Vec<Trace> }

impl Latency {
    pub fn begin_stroke(&mut self) { self.traces.clear(); }

    /// A sample made it to the screen.
    pub fn shown(&mut self, t: Trace) { if t.presented >= t.sampled { self.traces.push(t); } }

    /// The stroke's figures.
    pub fn stages(&self) -> Stages {
        let pick = |f: &dyn Fn(&Trace) -> u64| -> (u64, u64) {
            let mut v: Vec<u64> = self.traces.iter().map(f).collect();
            if v.is_empty() { return (0, 0); }
            v.sort_unstable();
            (v[v.len() / 2], v[v.len() - 1])
        };
        Stages {
            samples: self.traces.len(),
            input:   pick(&|t| t.received.since(t.sampled).0),
            paint:   pick(&|t| t.painted.since(t.received).0),
            wait:    pick(&|t| t.submitted.since(t.painted).0),
            present: pick(&|t| t.presented.since(t.submitted).0),
            total:   pick(&|t| t.presented.since(t.sampled).0),
        }
    }

    /// One line for the console and the report.
    pub fn line(&self) -> String {
        let s = self.stages();
        let ms = |v: (u64, u64)| format!("{:.1} / {:.1}", v.0 as f64 / 1000.0, v.1 as f64 / 1000.0);
        format!("pen → screen {} ms (median / worst, {} samples): input {}, paint {}, wait for frame {}, GPU + present {}", ms(s.total), s.samples, ms(s.input), ms(s.paint), ms(s.wait), ms(s.present))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_split_the_total() {
        let mut l = Latency::default();
        for i in 0..5u64 {
            l.shown(Trace { sampled: Micros(i * 1000), received: Micros(i * 1000 + 300), painted: Micros(i * 1000 + 2300), submitted: Micros(i * 1000 + 9000), presented: Micros(i * 1000 + 16000 + i * 100) });
        }
        let s = l.stages();
        assert_eq!((s.samples, s.input, s.paint.0, s.total), (5, (300, 300), 2000, (16200, 16400)));
        assert!(l.line().contains("pen → screen 16.2 / 16.4 ms"));
    }
}
