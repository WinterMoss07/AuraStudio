struct View {
    canvas_size: vec2<f32>,
    window_size: vec2<f32>,
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
    let cw = view.canvas_size.x;
    let ch = view.canvas_size.y;
    let ww = view.window_size.x;
    let wh = view.window_size.y;

    let scale = min(ww / cw, wh / ch);
    let nx = (cw * scale) / ww;
    let ny = (ch * scale) / wh;

    var pos = array<vec2<f32>, 6>(
        vec2(-nx, -ny), vec2( nx, -ny), vec2(-nx,  ny),
        vec2(-nx,  ny), vec2( nx, -ny), vec2( nx,  ny),
    );
    var uv = array<vec2<f32>, 6>(
        vec2(0.0, 1.0), vec2(1.0, 1.0), vec2(0.0, 0.0),
        vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(1.0, 0.0),
    );

    var out: VertexOutput;
    out.pos = vec4(pos[vi], 0.0, 1.0);
    out.uv  = uv[vi];
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_canvas, s_canvas, in.uv);
}
