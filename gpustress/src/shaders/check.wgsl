// Checker: run A vs run B bit-for-bit, plus CPU-known int results.

@group(0) @binding(0) var<storage, read> res_a: array<vec2<u32>>;
@group(0) @binding(1) var<storage, read> res_b: array<vec2<u32>>;
// (index, expected int-chain value), computed on the CPU.
@group(0) @binding(2) var<storage, read> samples: array<vec2<u32>>;
// [0] = A/B mismatches, [1] = CPU sample mismatches.
@group(0) @binding(3) var<storage, read_write> errs: array<atomic<u32>, 4>;

@compute @workgroup_size(256)
fn check(@builtin(global_invocation_id) gid3: vec3<u32>) {
    let i = gid3.x;
    if (i < arrayLength(&res_a)) {
        if (any(res_a[i] != res_b[i])) {
            atomicAdd(&errs[0], 1u);
        }
    }
    if (i < arrayLength(&samples)) {
        let s = samples[i];
        if (res_a[s.x].x != s.y || res_b[s.x].x != s.y) {
            atomicAdd(&errs[1], 1u);
        }
    }
}
