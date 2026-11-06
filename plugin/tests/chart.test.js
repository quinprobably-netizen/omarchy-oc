const test = require("node:test")
const assert = require("node:assert/strict")
const C = require("../lib/chart.js")

test("scaleOf: fixed bounds win, then the chart's, then the data's real min and max", () => {
  assert.deepEqual(C.scaleOf({ values: [5, 1, 9], min: 0, max: 100 }, null, null), { lo: 0, hi: 100 })
  assert.deepEqual(C.scaleOf({ values: [5, 1, 9] }, 0, 50), { lo: 0, hi: 50 })
  assert.deepEqual(C.scaleOf({ values: [5, 1, 9] }, null, null), { lo: 1, hi: 9 })
  // One bound given: the other comes from the data.
  assert.deepEqual(C.scaleOf({ values: [5, 1, 9], min: 0 }, null, null), { lo: 0, hi: 9 })
  assert.deepEqual(C.scaleOf({ values: [5, 1, 9] }, null, 20), { lo: 1, hi: 20 })
  // Gaps are skipped; a first finite sample does not freeze the bound.
  assert.deepEqual(C.scaleOf({ values: [NaN, 5, 1, 9, null, "7"] }, null, null), { lo: 1, hi: 9 })
})

test("scaleOf: flat data gets a band, nothing finite gives null", () => {
  assert.deepEqual(C.scaleOf({ values: [4, 4, 4] }, null, null), { lo: 3, hi: 5 })
  assert.equal(C.scaleOf({ values: [NaN, null] }, null, null), null)
  assert.equal(C.scaleOf({ values: [] }, null, null), null)
  assert.deepEqual(C.scaleOf({ values: [] }, 0, 100), { lo: 0, hi: 100 })
  assert.deepEqual(C.scaleOf({ values: [] }, "0", "100"), { lo: 0, hi: 100 })
})

test("sampleCount: the longest series sets the time axis", () => {
  assert.equal(C.sampleCount([{ values: [1, 2] }, { values: [1, 2, 3] }, {}]), 3)
  assert.equal(C.sampleCount([]), 0)
  assert.equal(C.sampleCount(null), 0)
})
