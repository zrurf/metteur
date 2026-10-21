import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { performance } from 'node:perf_hooks'
import { localDelayCommand, withCurrentNodePath } from './local-job-fixture.mjs'

test('the fixture runs a real quiet timer through the native command shell', () => {
  const start = performance.now()
  const result = spawnSync(localDelayCommand(25), {
    shell: true, windowsHide: true, env: withCurrentNodePath(), encoding: 'utf8', timeout: 10_000,
  })
  assert.ifError(result.error)
  assert.equal(result.status, 0, result.stderr)
  assert.equal(result.stdout, '')
  assert(performance.now() - start >= 20)
})

test('Windows PATH spelling is preserved and duplicate spellings are removed', () => {
  const original = { Path: 'C:\\Windows\\System32', PATH: 'ignored', OTHER: 'kept' }
  const result = withCurrentNodePath(original, 'D:\\Node Tools\\node.exe', 'win32')
  assert.equal(result.Path, 'D:\\Node Tools;C:\\Windows\\System32')
  assert.equal(result.PATH, undefined)
  assert.equal(result.OTHER, 'kept')
  assert.equal(original.PATH, 'ignored')
})

test('POSIX PATH is prefixed without changing unrelated case-sensitive variables', () => {
  const result = withCurrentNodePath({ PATH: '/usr/bin', Path: 'unrelated' }, '/opt/node/bin/node', 'linux')
  assert.equal(result.PATH, '/opt/node/bin:/usr/bin')
  assert.equal(result.Path, 'unrelated')
})

test('delay arguments cannot turn the local fixture into arbitrary shell input', () => {
  for (const value of [-1, 60_001, 1.5, NaN, '1; echo unsafe'])
    assert.throws(() => localDelayCommand(value), RangeError)
})
