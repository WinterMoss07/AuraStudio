// Canvas quad, placed by the view transform (zoom, rotation, pan).
struct View {
    // Rows of the canvas-pixel -> NDC affine matrix (xyz used).
    m0:          vec4<f32>,
    m1:          vec4<f32>,
    canvas_size: vec2<f32>,
    pad:         vec2<f32>,
}

@group(0) @binding(0) var t_canvas: texture_2d<f32>;
@group(0) @binding(1) var s_canvas: sampler;
@group(0) @binding(2) var<uniform> view: View;

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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_canvas, s_canvas, in.uv);
}
