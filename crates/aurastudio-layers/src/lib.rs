use aurastudio_canvas::{PaintSurface, Rect};

/// One layer: RGBA8, straight alpha, `width * height * 4` bytes.
pub struct Layer {
    /// Stable identity (thumbnails, UI state) that survives reordering.
    pub id:      u64,
    pub name:    String,
    pub visible: bool,
    pub opacity: f32,
    pub pixels:  Vec<u8>,
    /// The background layer is opaque white; clearing it resets it to
    /// white instead of transparent.
    pub background: bool,
}

/// Pixel a layer is cleared to: opaque white for the background,
/// transparent for the others. Transparent pixels keep a white RGB so a
/// compositing bug that ignores alpha shows white, not black.
fn clear_pixel(background: bool) -> [u8; 4] {
    if background { [255, 255, 255, 255] } else { [255, 255, 255, 0] }
}

impl Layer {
    pub fn new(width: u32, height: u32, background: bool) -> Self {
        let pixels = clear_pixel(background).repeat((width * height) as usize);
        Self { id: 0, name: "Layer".into(), visible: true, opacity: 1.0, pixels, background }
    }

    pub fn clear(&mut self) {
        let px = clear_pixel(self.background);
        for p in self.pixels.chunks_exact_mut(4) { p.copy_from_slice(&px); }
    }
}

/// Layer stack + active layer. Composite is on the CPU for now (straight
/// alpha "over", bottom to top, weighted by layer opacity): no blend
/// modes or GPU composite yet.
///
/// Memory: the per-stroke buffers (snapshot of the layer before the
/// stroke + stroke coverage) are only needed by the layer being painted,
/// so they live here once instead of in every layer. A layer costs 4
/// bytes per pixel instead of 9.
pub struct LayerStack {
    pub layers: Vec<Layer>,
    pub active: usize,
    pub width:  u32,
    pub height: u32,
    stroke_base: Vec<u8>,
    stroke_mask: Vec<u8>,
    next_id:     u64,
}

/// The active layer seen as a paintable surface for the brush engines.
pub struct ActiveSurface<'a> {
    pixels: &'a mut [u8],
    base:   &'a [u8],
    mask:   &'a mut [u8],
    width:  u32,
    height: u32,
}

impl PaintSurface for ActiveSurface<'_> {
    fn width(&self)  -> u32 { self.width }
    fn height(&self) -> u32 { self.height }
    fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) {
        (self.pixels, self.base, self.mask)
    }
}

impl LayerStack {
    pub fn new(width: u32, height: u32) -> Self {
        let mut base = Layer::new(width, height, true);
        base.name = "Background".into();
        base.id = 1;
        let n = (width * height) as usize;
        Self {
            layers: vec![base],
            active: 0,
            width,
            height,
            stroke_base: vec![0; n * 4],
            stroke_mask: vec![0; n],
            next_id: 2,
        }
    }

    fn insert_above_active(&mut self, mut layer: Layer) -> usize {
        layer.id = self.next_id;
        self.next_id += 1;
        // New layers go right above the active one, like Krita / CSP.
        let index = self.active + 1;
        self.layers.insert(index, layer);
        self.active = index;
        index
    }

    pub fn add_layer(&mut self, name: &str) -> usize {
        let mut layer = Layer::new(self.width, self.height, false);
        layer.name = name.into();
        self.insert_above_active(layer)
    }

    /// Copy of the active layer, placed above it and made active.
    pub fn duplicate_active(&mut self) -> usize {
        let src = &self.layers[self.active];
        let copy = Layer {
            id:         0,
            name:       format!("{} copy", src.name),
            visible:    src.visible,
            opacity:    src.opacity,
            pixels:     src.pixels.clone(),
            background: false,
        };
        self.insert_above_active(copy)
    }

    /// Move the active layer one step up (+1) or down (-1) in the stack.
    pub fn move_active(&mut self, up: bool) {
        let a = self.active;
        let b = if up { a + 1 } else { a.wrapping_sub(1) };
        if b < self.layers.len() {
            self.layers.swap(a, b);
            self.active = b;
        }
    }

    pub fn remove_layer(&mut self, index: usize) {
        if self.layers.len() <= 1 || index >= self.layers.len() { return; }
        self.layers.remove(index);
        // Removing a layer below the active one shifts every index above
        // it: keep following the same layer, not the same number.
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

    /// Erase the active layer's content (Delete key).
    pub fn clear_active(&mut self) {
        let a = self.active;
        self.layers[a].clear();
    }

    /// Call when a stroke starts: snapshot the active layer and reset the
    /// stroke coverage, so the new stroke never blends with itself but
    /// still paints over earlier strokes.
    pub fn begin_stroke(&mut self) {
        self.stroke_base.copy_from_slice(&self.layers[self.active].pixels);
        self.stroke_mask.fill(0);
    }

    pub fn active_surface(&mut self) -> ActiveSurface<'_> {
        ActiveSurface {
            pixels: &mut self.layers[self.active].pixels,
            base:   &self.stroke_base,
            mask:   &mut self.stroke_mask,
            width:  self.width,
            height: self.height,
        }
    }

    /// Change the canvas size, keeping the content centered (Krita /
    /// Photoshop "canvas size" with the default center anchor): growing
    /// adds empty margins, shrinking crops the borders. Nothing is scaled.
    pub fn resize(&mut self, width: u32, height: u32) {
        let (ow, oh) = (self.width as i64, self.height as i64);
        let (nw, nh) = (width as i64, height as i64);
        // Position of the old canvas's top-left corner in the new one.
        let (dx, dy) = ((nw - ow) / 2, (nh - oh) / 2);

        // Overlap, in old-canvas coordinates.
        let sx0 = (-dx).max(0);
        let sy0 = (-dy).max(0);
        let sx1 = (nw - dx).min(ow);
        let sy1 = (nh - dy).min(oh);

        for layer in &mut self.layers {
            let mut out = clear_pixel(layer.background).repeat((width * height) as usize);
            if sx0 < sx1 {
                for sy in sy0..sy1 {
                    let src = ((sy * ow + sx0) * 4) as usize..((sy * ow + sx1) * 4) as usize;
                    let ny = sy + dy;
                    let dst_start = ((ny * nw + sx0 + dx) * 4) as usize;
                    out[dst_start..dst_start + src.len()].copy_from_slice(&layer.pixels[src]);
                }
            }
            layer.pixels = out;
        }

        self.width  = width;
        self.height = height;
        let n = (width * height) as usize;
        self.stroke_base = vec![0; n * 4];
        self.stroke_mask = vec![0; n];
    }

    /// Composite every visible layer (bottom to top) into `out`, which must
    /// be `width * height * 4` bytes.
    pub fn composite_into(&self, out: &mut [u8]) {
        self.composite_rect_into(out, (0, 0, self.width, self.height));
    }

    /// Same as composite_into, limited to `rect`: avoids recompositing the
    /// whole canvas every frame while a stroke only touches a small area.
    /// `out` is still indexed as the full canvas; only `rect` is written.
    pub fn composite_rect_into(&self, out: &mut [u8], rect: Rect) {
        debug_assert_eq!(out.len(), (self.width * self.height * 4) as usize);
        let (x0, y0) = (rect.0, rect.1);
        let x1 = rect.2.min(self.width);
        let y1 = rect.3.min(self.height);
        if x0 >= x1 || y0 >= y1 { return; }
        let w = self.width;

        // White behind everything, in case every layer is hidden.
        for y in y0..y1 {
            let row = ((y * w + x0) * 4) as usize..((y * w + x1) * 4) as usize;
            out[row].fill(255);
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

    fn px(stack: &LayerStack, layer: usize, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * stack.width + x) * 4) as usize;
        stack.layers[layer].pixels[i..i + 4].try_into().unwrap()
    }

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
        stack.add_layer("Layer 2");
        let mut out = vec![0u8; 2 * 2 * 4];
        stack.composite_into(&mut out);
        assert_eq!(&out[0..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn removing_layer_below_keeps_same_active_layer() {
        let mut stack = LayerStack::new(2, 2);
        stack.add_layer("A");
        stack.add_layer("B"); // active = B (index 2)
        stack.remove_layer(1); // remove A, below it
        assert_eq!(stack.layers[stack.active].name, "B");
    }

    #[test]
    fn removing_the_top_active_layer() {
        let mut stack = LayerStack::new(2, 2);
        stack.add_layer("A");
        stack.remove_layer(1);
        assert_eq!(stack.layers.len(), 1);
        assert_eq!(stack.active, 0);
        stack.begin_stroke(); // must not panic with the new active index
    }

    #[test]
    fn duplicate_and_move() {
        let mut stack = LayerStack::new(2, 2);
        stack.add_layer("A");
        stack.layers[1].pixels[0..4].copy_from_slice(&[1, 2, 3, 255]);
        stack.duplicate_active();
        assert_eq!(stack.layers.len(), 3);
        assert_eq!(stack.active, 2);
        assert_eq!(stack.layers[2].name, "A copy");
        assert_eq!(&stack.layers[2].pixels[0..4], &[1, 2, 3, 255]);
        assert!(!stack.layers[2].background);
        // Ids stay unique.
        let ids: std::collections::HashSet<_> = stack.layers.iter().map(|l| l.id).collect();
        assert_eq!(ids.len(), 3);

        stack.move_active(false); // "A copy" goes below "A"
        assert_eq!(stack.active, 1);
        assert_eq!(stack.layers[1].name, "A copy");
        stack.move_active(true);
        stack.move_active(true); // already on top: no-op
        assert_eq!(stack.active, 2);
        stack.set_active(0);
        stack.move_active(false); // already at the bottom: no-op
        assert_eq!(stack.active, 0);
    }

    #[test]
    fn hidden_layer_is_skipped() {
        let mut stack = LayerStack::new(2, 2);
        stack.add_layer("Layer 2");
        stack.active_layer_mut().pixels[0..4].copy_from_slice(&[0, 0, 0, 255]);
        stack.active_layer_mut().visible = false;
        let mut out = vec![0u8; 2 * 2 * 4];
        stack.composite_into(&mut out);
        assert_eq!(&out[0..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn clear_resets_to_white_or_transparent() {
        let mut stack = LayerStack::new(2, 2);
        stack.active_layer_mut().pixels[0..4].copy_from_slice(&[0, 0, 0, 255]);
        stack.clear_active();
        assert_eq!(px(&stack, 0, 0, 0), [255, 255, 255, 255]);

        stack.add_layer("L");
        stack.active_layer_mut().pixels[0..4].copy_from_slice(&[0, 0, 0, 255]);
        stack.clear_active();
        assert_eq!(px(&stack, 1, 0, 0), [255, 255, 255, 0]);
    }

    #[test]
    fn resize_keeps_content_centered() {
        let mut stack = LayerStack::new(4, 4);
        stack.add_layer("L");
        // Mark pixel (1, 1) on the paint layer.
        let i = ((1 * 4 + 1) * 4) as usize;
        stack.layers[1].pixels[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);

        stack.resize(8, 8); // grows by 2 on each side
        assert_eq!((stack.width, stack.height), (8, 8));
        assert_eq!(px(&stack, 1, 3, 3), [0, 0, 0, 255]);
        assert_eq!(px(&stack, 1, 0, 0), [255, 255, 255, 0]); // new margin, transparent
        assert_eq!(px(&stack, 0, 0, 0), [255, 255, 255, 255]); // background margin, white

        stack.resize(4, 4); // back: crops the margins
        assert_eq!(px(&stack, 1, 1, 1), [0, 0, 0, 255]);

        stack.begin_stroke(); // stroke buffers follow the new size
        let s = stack.active_surface();
        assert_eq!(s.width() * s.height(), 16);
    }
}
