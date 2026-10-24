import { test } from 'node:test'
import assert from 'node:assert/strict'
import { fromWirePin } from '../src/core/node-catalog.ts'
test('loaded execution pins retain exact names and absent semantic keys', () => {
  for (const p of [
    { pinType: 'ExecInput', name: 'x-in', key: '', dataType: 'void' },
    { pinType: 'ExecOutput', name: 'x-out', key: '', dataType: 'void' },
    { pinType: 'DataInput', name: 'command', key: 'command', dataType: 'string' },
    { pinType: 'DataOutput', name: 'Result', key: '', dataType: 'any' },
  ]) {
    const actual = fromWirePin({ id: crypto.randomUUID(), ...p })
    assert.equal(actual.name, p.name)
    assert.equal(actual.key, p.key || undefined)
  }
})
