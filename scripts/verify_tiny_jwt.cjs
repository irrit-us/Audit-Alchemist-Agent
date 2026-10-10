// Verify the vendored implementation. Install the pinned dependencies documented
// in docs/evaluation.md and expose them through NODE_PATH; no fixture edits.
const assert = require('node:assert/strict')
const path = require('node:path')
const { createHmac } = require('node:crypto')
const createVerifier = require(path.resolve(__dirname, '../datasets/tiny/fast-jwt-iss/audit/src/verifier.js'))
const encode = value => Buffer.from(JSON.stringify(value)).toString('base64url')
const key = 'local-reproduction-secret'
const token = (payload, signed) => {
  const input = `${encode({ alg: signed ? 'HS256' : 'none' })}.${encode(payload)}`
  return input + '.' + (signed ? createHmac('sha256', key).update(input).digest('base64url') : '')
}
const unsigned = token({ sub: 'control' }, false)
assert.equal(createVerifier({ key: null, algorithms: ['none'] })(unsigned).sub, 'control')
assert.throws(() => createVerifier({ key, algorithms: ['HS256'] })(unsigned))
assert.throws(() => createVerifier({ key: null, algorithms: ['HS256'] })(unsigned))
const emptySigned = `${encode({ alg: 'HS256' })}.${encode({ sub: 'forged' })}.`
assert.equal(createVerifier({ key: null, algorithms: ['HS256'] })(emptySigned).sub, 'forged')
assert.throws(() => createVerifier({ key, algorithms: ['HS256'] })(emptySigned))
const realNow = Date.now
let now = 1000000
Date.now = () => now
try {
  const cached = createVerifier({ key, algorithms: ['HS256'], cache: true })
  const uncached = createVerifier({ key, algorithms: ['HS256'] })
  const noIat = token({ sub: 'expiry', exp: 1001 }, true)
  const withIat = token({ sub: 'expiry', iat: 999, exp: 1001 }, true)
  cached(noIat)
  cached(withIat)
  now = 1002000
  assert.equal(cached(noIat).sub, 'expiry')
  assert.throws(() => uncached(noIat), error => error.code === 'FAST_JWT_EXPIRED')
  assert.throws(() => cached(withIat), error => error.code === 'FAST_JWT_EXPIRED')
  now = 1000000
  const maxAgeCached = createVerifier({ key, algorithms: ['HS256'], cache: true, maxAge: 1000 })
  const maxAgeUncached = createVerifier({ key, algorithms: ['HS256'], maxAge: 1000 })
  const oldToken = token({ sub: 'maxAge', iat: 1000, exp: 1600 }, true)
  maxAgeCached(oldToken)
  now = 1002000
  assert.equal(maxAgeCached(oldToken).sub, 'maxAge')
  assert.throws(() => maxAgeUncached(oldToken), error => error.code === 'FAST_JWT_EXPIRED')
  console.log(JSON.stringify({ unsigned_opt_in: 'accepted as explicitly configured',
    unsigned_with_key: 'rejected', unsigned_disallowed_algorithm: 'rejected',
    empty_hs256_with_null_key: 'accepted: signed-algorithm verification bypass under invalid key configuration',
    empty_hs256_with_real_key: 'rejected',
    expired_signed_no_iat_cached: 'accepted: confirmed expiry bypass',
    expired_signed_no_iat_uncached: 'rejected', expired_signed_with_iat_cached: 'rejected',
    signed_max_age_cached: 'accepted after maxAge: confirmed lifetime bypass when exp is later',
    signed_max_age_uncached: 'rejected',
    scope: 'cache:true required; no universal authentication bypass; default cacheTTL is 600000 ms' }, null, 2))
} finally {
  Date.now = realNow
}
