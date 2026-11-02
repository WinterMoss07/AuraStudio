// Canvas quad, placed by the view transform (zoom, rotation, pan).
struct View {
    // Rows of the canvas-pixel -> NDC affine matrix (xyz used).
    m0:          vec4<f32>,
    m1:          vec4<f32>,
    canvas_size: vec2<f32>,
    pad:         vec2<f32>,
    // Display color management: x = on (1) / off (0), y = LUT size.
    cm:          vec4<f32>,
    // Rows of the screen-pixel -> canvas-pixel affine matrix (xyz used):
    // canvas x = dot(inv0.xyz, (px, py, 1)).
    inv0:        vec4<f32>,
    inv1:        vec4<f32>,
    // x: the view is rotated (not by a multiple of 90°): the footprint of
    // a screen pixel is a tilted square; w: smooth zoom-in (bilinear).
    q:           vec4<f32>,
}

@group(0) @binding(0) var t_canvas: texture_2d<f32>;
@group(0) @binding(1) var s_canvas: sampler;
@group(0) @binding(2) var<uniform> view: View;
// sRGB -> monitor lookup table (3D: r, g, b), see aurastudio-color.
@group(0) @binding(3) var t_lut: texture_3d<f32>;
@group(0) @binding(4) var s_lut: sampler;

struct VertexOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0)       uv:  vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    // Two triangles covering the canvas, in canvas pixels.
    var uv = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let p = vec3(uv[vi] * view.canvas_size, 1.0);

    var out: VertexOutput;
    out.pos = vec4(dot(view.m0.xyz, p), dot(view.m1.xyz, p), 0.0, 1.0);
    out.uv  = uv[vi];
    return out;
}

// The monitor's colors, when color management is on.
fn display(c: vec4<f32>) -> vec4<f32> {
    // Texel centers: n points span 0..1.
    let n = max(view.cm.y, 2.0);
    let uvw = c.rgb * ((n - 1.0) / n) + vec3(0.5 / n);
    let managed = textureSampleLevel(t_lut, s_lut, uvw, 0.0).rgb;
    return vec4(select(c.rgb, managed, view.cm.x > 0.5), c.a);
}

// Standard: the mipmaps (trilinear), or nearest close up (the sampler).
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return display(textureSample(t_canvas, s_canvas, in.uv));
}

// --- High: the exact average of the canvas under each screen pixel ------------

fn canvas_at(p: vec2<f32>) -> vec2<f32> {
    let q = vec3(p, 1.0);
    return vec2(dot(view.inv0.xyz, q), dot(view.inv1.xyz, q));
}

fn cross2(a: vec2<f32>, b: vec2<f32>) -> f32 { return a.x * b.y - a.y * b.x; }

// Which side of the edge e0 -> e1 the point p is on (>= 0: inside, with
// `s` = +1 or -1 the quad's turning sense).
fn side(e0: vec2<f32>, e1: vec2<f32>, p: vec2<f32>, s: f32) -> f32 { return s * cross2(e1 - e0, p - e0); }

// The part [t0, t1] of a -> b kept by one edge of the quad.
fn keep(t: vec2<f32>, e0: vec2<f32>, e1: vec2<f32>, a: vec2<f32>, b: vec2<f32>, s: f32) -> vec2<f32> {
    let f0 = side(e0, e1, a, s);
    let f1 = side(e0, e1, b, s);
    if (f1 == f0) { return select(t, vec2(1.0, 0.0), f0 < 0.0); }
    let r = -f0 / (f1 - f0);
    if (f1 > f0) { return vec2(max(t.x, r), t.y); }
    return vec2(t.x, min(t.y, r));
}

// One bound of the unit square for a segment (Liang-Barsky: pk t <= qk).
fn bound(t: vec2<f32>, pk: f32, qk: f32) -> vec2<f32> {
    if (pk == 0.0) { return select(t, vec2(1.0, 0.0), qk < 0.0); }
    let r = qk / pk;
    if (pk < 0.0) { return vec2(max(t.x, r), t.y); }
    return vec2(t.x, min(t.y, r));
}

// The integral of x dy along the part [t.x, t.y] of a -> b.
fn x_dy(a: vec2<f32>, b: vec2<f32>, t: vec2<f32>) -> f32 {
    if (t.x >= t.y) { return 0.0; }
    let p = a + (b - a) * t.x;
    let q = a + (b - a) * t.y;
    return (p.x + q.x) * 0.5 * (q.y - p.y);
}

// x dy along the part of the quad's edge a -> b inside the unit square.
fn edge_in_square(a: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = b - a;
    var t = vec2(0.0, 1.0);
    t = bound(t, -d.x, a.x);
    t = bound(t, d.x, 1.0 - a.x);
    t = bound(t, -d.y, a.y);
    t = bound(t, d.y, 1.0 - a.y);
    return x_dy(a, b, t);
}

// x dy along the part of the square's edge a -> b inside the quad.
fn edge_in_quad(a: vec2<f32>, b: vec2<f32>, c0: vec2<f32>, c1: vec2<f32>, c2: vec2<f32>, c3: vec2<f32>, s: f32) -> f32 {
    var t = vec2(0.0, 1.0);
    t = keep(t, c0, c1, a, b, s);
    t = keep(t, c1, c2, a, b, s);
    t = keep(t, c2, c3, a, b, s);
    t = keep(t, c3, c0, a, b, s);
    return x_dy(a, b, t);
}

// Area of the unit square at the origin inside the convex quad c0..c3
// (relative to the square; `s` its turning sense): Green's theorem, the
// integral of x dy around the intersection, which is made of the quad's
// edges inside the square and the square's edges inside the quad. (No
// arrays: some drivers fail on indexed local arrays.)
fn clipped_area(c0: vec2<f32>, c1: vec2<f32>, c2: vec2<f32>, c3: vec2<f32>, s: f32) -> f32 {
    let quad = edge_in_square(c0, c1) + edge_in_square(c1, c2) + edge_in_square(c2, c3) + edge_in_square(c3, c0);
    let square = edge_in_quad(vec2(0.0, 0.0), vec2(1.0, 0.0), c0, c1, c2, c3, s)
               + edge_in_quad(vec2(1.0, 0.0), vec2(1.0, 1.0), c0, c1, c2, c3, s)
               + edge_in_quad(vec2(1.0, 1.0), vec2(0.0, 1.0), c0, c1, c2, c3, s)
               + edge_in_quad(vec2(0.0, 1.0), vec2(0.0, 0.0), c0, c1, c2, c3, s);
    // The square turns positively; walked in the quad's sense, its part
    // counts with the quad's sign.
    return abs(quad + s * square);
}

// The least and greatest side of the square at o (unit) for one edge.
fn span(e0: vec2<f32>, e1: vec2<f32>, o: vec2<f32>, s: f32) -> vec2<f32> {
    let a0 = side(e0, e1, o, s);
    let a1 = side(e0, e1, o + vec2(1.0, 0.0), s);
    let a2 = side(e0, e1, o + vec2(1.0, 1.0), s);
    let a3 = side(e0, e1, o + vec2(0.0, 1.0), s);
    return vec2(min(min(a0, a1), min(a2, a3)), max(max(a0, a1), max(a2, a3)));
}

// Every canvas pixel under the screen pixel at `p` (its center), weighted
// by the part of it covered: canvas pixels are squares of one color, the
// screen pixel a square seen through the view. Divided by the area covered
// (the part of the screen pixel over the canvas).
fn exact(p: vec2<f32>) -> vec4<f32> {
    let c0 = canvas_at(p + vec2(-0.5, -0.5));
    let c1 = canvas_at(p + vec2(0.5, -0.5));
    let c2 = canvas_at(p + vec2(0.5, 0.5));
    let c3 = canvas_at(p + vec2(-0.5, 0.5));
    let size = view.canvas_size;
    let lo = clamp(min(min(c0, c1), min(c2, c3)), vec2(0.0), size);
    let hi = clamp(max(max(c0, c1), max(c2, c3)), vec2(0.0), size);
    let i0 = vec2<i32>(floor(lo));
    let i1 = vec2<i32>(ceil(hi));
    var sum = vec4(0.0);
    var total = 0.0;
    if (view.q.x < 0.5) {
        // Upright: the footprint is a box, each pixel weighs the product
        // of its overlaps along x and y.
        for (var j = i0.y; j < i1.y; j++) {
            let wy = min(f32(j + 1), hi.y) - max(f32(j), lo.y);
            if (wy <= 0.0) { continue; }
            for (var i = i0.x; i < i1.x; i++) {
                let wx = min(f32(i + 1), hi.x) - max(f32(i), lo.x);
                if (wx <= 0.0) { continue; }
                let w = wx * wy;
                sum += textureLoad(t_canvas, vec2(i, j), 0) * w;
                total += w;
            }
        }
    } else {
        // Tilted: pixels wholly inside weigh 1, those on the edges the
        // part the quad covers.
        let s = select(-1.0, 1.0, cross2(c1 - c0, c3 - c0) >= 0.0);
        for (var j = i0.y; j < i1.y; j++) {
            for (var i = i0.x; i < i1.x; i++) {
                let o = vec2(f32(i), f32(j));
                let k0 = span(c0, c1, o, s);
                let k1 = span(c1, c2, o, s);
                let k2 = span(c2, c3, o, s);
                let k3 = span(c3, c0, o, s);
                if (k0.y < 0.0 || k1.y < 0.0 || k2.y < 0.0 || k3.y < 0.0) { continue; }
                var w = 1.0;
                if (min(min(k0.x, k1.x), min(k2.x, k3.x)) < 0.0) {
                    w = clipped_area(c0 - o, c1 - o, c2 - o, c3 - o, s);
                }
                if (w <= 0.0) { continue; }
                sum += textureLoad(t_canvas, vec2(i, j), 0) * w;
                total += w;
            }
        }
    }
    if (total <= 0.0) { return vec4(0.0); }
    return sum / total;
}

@fragment
fn fs_high(in: VertexOutput) -> @location(0) vec4<f32> {
    var c: vec4<f32>;
    if (view.q.w > 0.5) {
        // Close up, smooth: bilinear between canvas pixels.
        c = textureSampleLevel(t_canvas, s_canvas, in.uv, 0.0);
    } else {
        c = exact(in.pos.xy);
    }
    return display(c);
}
