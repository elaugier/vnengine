// Textured quad shader for sprites and backgrounds.
// @@FX_DECL is replaced at pipeline-creation time with the transition
// FX declaration (`var<immediate>` + mask helpers) when the device
// supports immediates, or with nothing otherwise.

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(in.pos, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

// @@FX_DECL

@group(0) @binding(0) var t_diffuse: texture_2d<f32>;
@group(0) @binding(1) var s_diffuse: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var uv = in.uv;
    var cm = vec4<f32>(1.0);
    // @@FX_BODY
    let tex = textureSample(t_diffuse, s_diffuse, uv);
    return vec4<f32>(tex.rgb * in.color.rgb * cm.rgb, tex.a * in.color.a * cm.a);
}
