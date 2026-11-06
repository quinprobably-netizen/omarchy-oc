// VRAM pattern fill + on-GPU verification.
//
// Each chunk buffer is an array of vec4<u32>; the pattern value of a 32-bit word
// depends only on its global word address, the pattern id and a seed, so verify
// can recompute it without a reference copy.

struct Params {
    // Global vec4 index of element 0 of this chunk.
    base: u32,
    pattern: u32,
    seed: u32,
    // Fault injection (0xffffffff = off): local vec4 index written wrong.
    inject: u32,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> mem: array<vec4<u32>>;
// [0] = mismatching words.
@group(0) @binding(2) var<storage, read_write> errs: array<atomic<u32>, 4>;

fn mix32(x: u32) -> u32 {
    var h = x;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

fn pat(w: u32) -> u32 {
    switch (p.pattern) {
        // walking ones / walking zeros
        case 0u: { return 1u << ((w + p.seed) & 31u); }
        case 1u: { return ~(1u << ((w + p.seed) & 31u)); }
        // address XOR seed / inverted
        case 2u: { return w ^ p.seed; }
        case 3u: { return ~(w ^ p.seed); }
        // pseudo-random / inverted
        case 4u: { return mix32(w ^ p.seed); }
        default: { return ~mix32(w ^ p.seed); }
    }
}

fn expected(i: u32) -> vec4<u32> {
    let w = (p.base + i) * 4u;
    return vec4<u32>(pat(w), pat(w + 1u), pat(w + 2u), pat(w + 3u));
}

@compute @workgroup_size(256)
fn fill(@builtin(global_invocation_id) gid3: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let n = arrayLength(&mem);
    let stride = nwg.x * 256u;
    for (var i = gid3.x; i < n; i += stride) {
        var v = expected(i);
        if (i == p.inject) {
            v.x ^= 0x00010000u;
        }
        mem[i] = v;
    }
}

@compute @workgroup_size(256)
fn verify(@builtin(global_invocation_id) gid3: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let n = arrayLength(&mem);
    let stride = nwg.x * 256u;
    var bad = 0u;
    for (var i = gid3.x; i < n; i += stride) {
        let d = mem[i] != expected(i);
        bad += u32(d.x) + u32(d.y) + u32(d.z) + u32(d.w);
    }
    if (bad != 0u) {
        atomicAdd(&errs[0], bad);
    }
}
