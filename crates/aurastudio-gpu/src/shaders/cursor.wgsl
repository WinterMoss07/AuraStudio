// Brush outline under the pen: the outline of the dab the brush would
// lay (a round tip's ellipse, or the 50 % contour of a sampled tip's
// picture), anti-aliased, light inside a dark halo (visible on any color);
// a small crosshair when the brush is too small to outline. Drawn over
// everything.
struct Cursor {
    center:      vec2<f32>,  // physical window pixels
    screen:      vec2<f32>,  // surface size, pixels
    to_tip:      vec4<f32>,  // window offset -> tip coordinates (2 x 2, row-major)
    mask_scale:  vec2<f32>,  // tip -1..1 -> the picture inside its clear border
    mask_offset: vec2<f32>,
    extent:      f32,        // how far it reaches, window pixels
    mode:        f32,        // 0 round, 1 picture, 2 crosshair
    pad:         vec2<f32>,
}

@group(0) @binding(0) var<uniform> cur: Cursor;
@group(0) @binding(1) var mask: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    );
    let half_size = select(cur.extent + 3.0, 10.0, cur.mode > 1.5);
    let p = cur.center + corners[vi] * half_size;
    let ndc = vec2(p.x / cur.screen.x * 2.0 - 1.0, 1.0 - p.y / cur.screen.y * 2.0);
    return vec4(ndc, 0.0, 1.0);
}

// Coverage of a 1 px line at signed distance d.
fn line(d: f32) -> f32 {
    return clamp(1.0 - abs(d), 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = frag.xy - cur.center;
    // The outline: where f crosses 0 (positive inside), in window pixels
    // through its rate of change (both computed everywhere: the
    // derivatives need it).
    let t = vec2(cur.to_tip.x * p.x + cur.to_tip.y * p.y, cur.to_tip.z * p.x + cur.to_tip.w * p.y);
    let round = 1.0 - length(t);
    let uv = (t * 0.5 + 0.5) * cur.mask_scale + cur.mask_offset;
    let picture = textureSampleLevel(mask, samp, uv, 0.0).r - 0.5;
    let f = select(round, picture, cur.mode > 0.5);
    let dist = f / max(fwidth(f), 1e-5);
    var light = line(dist);
    var dark = line(dist + 1.0);
    // Short cross with a gap in the middle.
    let ax = abs(p.x);
    let ay = abs(p.y);
    let cross_light = max(line(p.y) * step(2.5, ax) * step(ax, 7.5), line(p.x) * step(2.5, ay) * step(ay, 7.5));
    let cross_dark = max(line(abs(p.y) - 1.0) * step(2.5, ax) * step(ax, 8.5),
                         line(abs(p.x) - 1.0) * step(2.5, ay) * step(ay, 8.5));
    if (cur.mode > 1.5) {
        light = cross_light;
        dark = cross_dark;
    }
    // Light line drawn over the dark halo.
    let a = max(light, dark * 0.65);
    let c = select(vec3(0.0), vec3(1.0), light >= dark * 0.65);
    return vec4(c * a, a);
}
