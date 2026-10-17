use aurastudio_canvas::{PaintSurface, Rect};

/// Un calque : son propre buffer RGBA8 + son propre état de trait en
/// cours (stroke_base/stroke_mask), pour que le fix anti double-blend
/// du brush engine s'applique par calque, pas globalement.
pub struct Layer {
    pub name:        String,
    pub visible:     bool,
    pub opacity:     f32,
    pub pixels:      Vec<u8>,
    pub stroke_base: Vec<u8>,
    pub stroke_mask: Vec<u8>,
    pub width:       u32,
    pub height:      u32,
}

impl Layer {
    /// transparent=false : calque opaque blanc (fond). transparent=true :
    /// calque vide (alpha 0) pour dessiner par-dessus sans cacher le dessous.
    pub fn new(width: u32, height: u32, transparent: bool) -> Self {
        let fill = if transparent { 0u8 } else { 255u8 };
        let mut pixels = vec![fill; (width * height * 4) as usize];
        if transparent {
            // RGB neutre, alpha à 0 : évite un comportement "noir qui
            // apparaît" si jamais la composition ignore l'alpha par erreur.
            for px in pixels.chunks_exact_mut(4) {
                px[0] = 255; px[1] = 255; px[2] = 255; px[3] = 0;
            }
        }
        let stroke_base = pixels.clone();
        let stroke_mask = vec![0u8; (width * height) as usize];
        Self { name: "Calque".into(), visible: true, opacity: 1.0, pixels, stroke_base, stroke_mask, width, height }
    }

    pub fn begin_stroke(&mut self) {
        self.stroke_base.copy_from_slice(&self.pixels);
        self.stroke_mask.iter_mut().for_each(|m| *m = 0);
    }
}

impl PaintSurface for Layer {
    fn width(&self)  -> u32 { self.width }
    fn height(&self) -> u32 { self.height }
    fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) {
        (&mut self.pixels, &self.stroke_base, &mut self.stroke_mask)
    }
}

/// Pile de calques + index du calque actif. v1 : composite CPU simple
/// (alpha "over" de bas en haut, pondéré par l'opacité du calque) —
/// pas encore de blend modes ni de composite GPU.
pub struct LayerStack {
    pub layers: Vec<Layer>,
    pub active: usize,
    pub width:  u32,
    pub height: u32,
}

impl LayerStack {
    pub fn new(width: u32, height: u32) -> Self {
        let mut base = Layer::new(width, height, false);
        base.name = "Fond".into();
        Self { layers: vec![base], active: 0, width, height }
    }

    pub fn add_layer(&mut self, name: &str) -> usize {
        let mut layer = Layer::new(self.width, self.height, true);
        layer.name = name.into();
        self.layers.push(layer);
        self.active = self.layers.len() - 1;
        self.active
    }

    pub fn remove_layer(&mut self, index: usize) {
        if self.layers.len() <= 1 || index >= self.layers.len() { return; }
        self.layers.remove(index);
        // Un calque supprimé sous l'actif décale tous les index au-dessus :
        // on suit le même calque, pas le même numéro.
        if index < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.layers.len() - 1);
    }

    pub fn set_active(&mut self, index: usize) {
        if index < self.layers.len() { self.active = index; }
    }

    pub fn active_layer_mut(&mut self) -> &mut Layer {
        &mut self.layers[self.active]
    }

    /// Composite tous les calques visibles (bas vers haut) dans `out`,
    /// qui doit faire width*height*4 octets. Alpha "over" standard.
    pub fn composite_into(&self, out: &mut [u8]) {
        debug_assert_eq!(out.len(), (self.width * self.height * 4) as usize);
        out.fill(255); // fond blanc par défaut si tous les calques sont masqués

        for layer in &self.layers {
            if !layer.visible || layer.opacity <= 0.0 { continue; }
            for (dst, src) in out.chunks_exact_mut(4).zip(layer.pixels.chunks_exact(4)) {
                let src_a = (src[3] as f32 / 255.0) * layer.opacity;
                if src_a <= 0.0 { continue; }
                let ia = 1.0 - src_a;
                dst[0] = (src[0] as f32 * src_a + dst[0] as f32 * ia) as u8;
                dst[1] = (src[1] as f32 * src_a + dst[1] as f32 * ia) as u8;
                dst[2] = (src[2] as f32 * src_a + dst[2] as f32 * ia) as u8;
                dst[3] = ((src_a + (dst[3] as f32 / 255.0) * ia) * 255.0) as u8;
            }
        }
    }

    /// Comme composite_into, mais limité à `rect` — évite de recomposer
    /// les 2048×2048 pixels à chaque échantillon de pointeur pendant un
    /// trait. `out` doit toujours faire width*height*4 (indexation
    /// globale), seule la zone `rect` est réécrite.
    pub fn composite_rect_into(&self, out: &mut [u8], rect: Rect) {
        let (x0, y0, x1, y1) = rect;
        let w = self.width;

        for y in y0..y1 {
            let row = ((y * w + x0) * 4) as usize..((y * w + x1) * 4) as usize;
            for px in out[row.clone()].chunks_exact_mut(4) { px.copy_from_slice(&[255, 255, 255, 255]); }
        }

        for layer in &self.layers {
            if !layer.visible || layer.opacity <= 0.0 { continue; }
            for y in y0..y1 {
                let row = ((y * w + x0) * 4) as usize..((y * w + x1) * 4) as usize;
                let dst_row = &mut out[row.clone()];
                let src_row = &layer.pixels[row];
                for (dst, src) in dst_row.chunks_exact_mut(4).zip(src_row.chunks_exact(4)) {
                    let src_a = (src[3] as f32 / 255.0) * layer.opacity;
                    if src_a <= 0.0 { continue; }
                    let ia = 1.0 - src_a;
                    dst[0] = (src[0] as f32 * src_a + dst[0] as f32 * ia) as u8;
                    dst[1] = (src[1] as f32 * src_a + dst[1] as f32 * ia) as u8;
                    dst[2] = (src[2] as f32 * src_a + dst[2] as f32 * ia) as u8;
                    dst[3] = ((src_a + (dst[3] as f32 / 255.0) * ia) * 255.0) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_flattens_opaque_base() {
        let stack = LayerStack::new(2, 2);
        let mut out = vec![0u8; 2 * 2 * 4];
        stack.composite_into(&mut out);
        assert_eq!(&out[0..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn transparent_layer_does_not_hide_base() {
        let mut stack = LayerStack::new(2, 2);
        stack.add_layer("Calque 2");
        let mut out = vec![0u8; 2 * 2 * 4];
        stack.composite_into(&mut out);
        assert_eq!(&out[0..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn removing_layer_below_keeps_same_active_layer() {
        let mut stack = LayerStack::new(2, 2);
        stack.add_layer("A");
        stack.add_layer("B"); // actif = B (index 2)
        stack.remove_layer(1); // supprime A, en dessous
        assert_eq!(stack.layers[stack.active].name, "B");
    }

    #[test]
    fn hidden_layer_is_skipped() {
        let mut stack = LayerStack::new(2, 2);
        stack.add_layer("Calque 2");
        stack.active_layer_mut().pixels[0..4].copy_from_slice(&[0, 0, 0, 255]);
        stack.active_layer_mut().visible = false;
        let mut out = vec![0u8; 2 * 2 * 4];
        stack.composite_into(&mut out);
        assert_eq!(&out[0..4], &[255, 255, 255, 255]);
    }
}
