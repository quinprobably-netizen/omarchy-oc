// Heavy deterministic ALU kernel.
//
// Every invocation runs two independent dependency chains for `rounds` rounds:
//   * an integer hash chain (bijective xorshift-multiply steps): its final value
//     is reproduced on the CPU for a sample of indices;
//   * 32 lanes of the chaotic quadratic map y <- (A*y + B)*y + c, written as two
//     FMAs. It is conjugate to z <- z*z + C with C just above -2 (bounded and
//     fully chaotic), and not symmetric around 0, so a corrupted bit in an
//     intermediate value is amplified instead of forgotten (only flips in the
//     lowest mantissa bits can be legitimately rounded away, as in any float
//     workload). Results are compared bit-for-bit between two identical GPU
//     runs, never against the CPU (FMA rounding may differ).

struct Params {
    seed: u32,
    rounds: u32,
    // Fault injection (0xffffffff = off): corrupt the int chain start of this index.
    inject_int: u32,
    // Fault injection (0xffffffff = off): corrupt the float chain start of this index.
    inject_float: u32,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> outp: array<vec2<u32>>;

fn mix32(x: u32) -> u32 {
    var h = x;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

fn unit(h: u32) -> f32 {
    // [-1.9, 1.9): inside the invariant interval of every map below.
    return f32(h >> 8u) * (3.8 / 16777216.0) - 1.9;
}

fn init4(base: u32) -> vec4<f32> {
    return vec4<f32>(unit(mix32(base)), unit(mix32(base + 1u)), unit(mix32(base + 2u)), unit(mix32(base + 3u)));
}

fn fold(v: vec4<f32>) -> u32 {
    let b = bitcast<vec4<u32>>(v);
    return b.x ^ ((b.y << 7u) | (b.y >> 25u)) ^ ((b.z << 13u) | (b.z >> 19u)) ^ ((b.w << 21u) | (b.w >> 11u));
}

@compute @workgroup_size(256)
fn stress(@builtin(global_invocation_id) gid3: vec3<u32>) {
    let gid = gid3.x;
    if (gid >= arrayLength(&outp)) {
        return;
    }
    var x = mix32(gid ^ mix32(p.seed));
    if (gid == p.inject_int) {
        x ^= 1u;
    }
    let base = mix32(x ^ 0xa5a5a5a5u);
    var v0 = init4(base);
    var v1 = init4(base + 4u);
    var v2 = init4(base + 8u);
    var v3 = init4(base + 12u);
    var v4 = init4(base + 16u);
    var v5 = init4(base + 20u);
    var v6 = init4(base + 24u);
    var v7 = init4(base + 28u);
    if (gid == p.inject_float) {
        v0.x = bitcast<f32>(bitcast<u32>(v0.x) ^ 0x00400000u);
    }
    // Per-lane c (C = 0.75*c + 0.1875 in [-1.99990, -1.99859]).
    let ca = vec4<f32>(-2.9165332, -2.9163067, -2.9160132, -2.9157600);
    let cb = vec4<f32>(-2.9155600, -2.9152534, -2.9150267, -2.9147866);
    let a = 0.75;
    let b = 0.5;
    for (var r = 0u; r < p.rounds; r++) {
        x = mix32(x + 0x9e3779b9u);
        v0 = fma(fma(v0, vec4(a), vec4(b)), v0, ca);
        v1 = fma(fma(v1, vec4(a), vec4(b)), v1, cb);
        v2 = fma(fma(v2, vec4(a), vec4(b)), v2, ca);
        v3 = fma(fma(v3, vec4(a), vec4(b)), v3, cb);
        v4 = fma(fma(v4, vec4(a), vec4(b)), v4, ca);
        v5 = fma(fma(v5, vec4(a), vec4(b)), v5, cb);
        v6 = fma(fma(v6, vec4(a), vec4(b)), v6, ca);
        v7 = fma(fma(v7, vec4(a), vec4(b)), v7, cb);
    }
    let f = fold(v0) ^ (fold(v1) * 3u) ^ (fold(v2) * 5u) ^ (fold(v3) * 7u)
          ^ (fold(v4) * 9u) ^ (fold(v5) * 11u) ^ (fold(v6) * 13u) ^ (fold(v7) * 15u);
    outp[gid] = vec2<u32>(x, f);
}

