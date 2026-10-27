// 全屏三角形 + 像素纹理采样。
//
// 顶点着色器不接收顶点缓冲，只按 vertex_index 生成覆盖整个裁剪空间的三角形；
// 片元着色器直接采样 CPU 光栅出来的那一帧（预乘 alpha）。

struct Params {
    /// 1.0 = 表面是 PostMultiplied，需要把预乘颜色还原成直通 alpha。
    unpremultiply: f32,
    _pad: vec3<f32>,
}

@group(0) @binding(0) var frame_tex: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 2.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(2.0, 0.0),
    );

    var out: VsOut;
    out.pos = vec4<f32>(positions[vi], 0.0, 1.0);
    out.uv = uvs[vi];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(frame_tex, frame_sampler, in.uv);
    if (params.unpremultiply > 0.5) {
        return vec4<f32>(c.rgb / max(c.a, 0.0001), c.a);
    }
    return c;
}