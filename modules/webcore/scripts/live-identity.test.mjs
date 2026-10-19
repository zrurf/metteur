import { test } from 'node:test'
import assert from 'node:assert/strict'
import { assertLiveIdentity } from './live-identity.mjs'

test('live preflight rejects a different application', () => {
  assert.throws(() => assertLiveIdentity('<title>Another app</title>'), /Metteur application/)
})
test('live preflight rejects mock and unlabelled Metteur builds', () => {
  for (const marker of ['', '<meta name="metteur-gateway" content="mock">']) {
    assert.throws(() => assertLiveIdentity(`<title>Metteur</title>${marker}`), /grpc gateway/)
  }
})
test('live preflight accepts only the labelled grpc build', () => {
  assert.doesNotThrow(() =>
    assertLiveIdentity('<title>Metteur</title><meta name="metteur-gateway" content="grpc">'),
  )
})
