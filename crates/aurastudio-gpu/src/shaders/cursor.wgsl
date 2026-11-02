// Brush outline under the pen: an anti-aliased ring the size of the brush
// (light inside, dark outside, visible on any color), or a small crosshair
// when the brush is too small to outline. Drawn over everything.
struct Cursor {
    center: vec2<f32>,  // physical window pixels
    radius: f32,        // screen pixels
    cross:  f32,        // 1 = crosshair instead of ring
    screen: vec2<f32>,  // surface size, pixels
    pad:    vec2<f32>,
}

@group(0) @binding(0) var<uniform> cur: Cursor;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    );
    let half_size = select(cur.radius + 3.0, 10.0, cur.cross > 0.5);
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
    var light = 0.0;
    var dark = 0.0;
    if (cur.cross > 0.5) {
        // Short cross with a gap in the middle.
        let ax = abs(p.x);
        let ay = abs(p.y);
        let on_h = line(p.y) * step(2.5, ax) * step(ax, 7.5);
        let on_v = line(p.x) * step(2.5, ay) * step(ay, 7.5);
        light = max(on_h, on_v);
        dark = max(line(abs(p.y) - 1.0) * step(2.5, ax) * step(ax, 8.5),
                   line(abs(p.x) - 1.0) * step(2.5, ay) * step(ay, 8.5));
    } else {
        let d = length(p);
        light = line(d - cur.radius);
        dark = line(d - cur.radius - 1.0);
    }
    // Light line drawn over the dark halo.
    let a = max(light, dark * 0.65);
    let c = select(vec3(0.0), vec3(1.0), light >= dark * 0.65);
    return vec4(c * a, a);
}
