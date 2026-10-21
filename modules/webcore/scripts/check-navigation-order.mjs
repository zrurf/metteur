import assert from 'node:assert/strict'
import { normalizeOrder,moveRelative,moveStep } from '../src/components/navigation/order.ts'
const all=['chat','explorer','execution','tasks']
assert.deepEqual(normalizeOrder(null,all),all)
assert.deepEqual(normalizeOrder(['tasks','chat','chat','unknown'],all),['tasks','chat','explorer','execution'])
assert.deepEqual(moveRelative(all,'tasks','chat',false),['tasks','chat','explorer','execution'])
assert.deepEqual(moveRelative(all,'chat','execution',true),['explorer','execution','chat','tasks'])
assert.deepEqual(moveRelative(all,'chat','chat',true),all)
assert.deepEqual(moveRelative(all,'unknown','chat',true),all)
assert.deepEqual(moveStep(all,'chat',-1),all)
assert.deepEqual(moveStep(all,'tasks',1),all)
assert.deepEqual(moveStep(all,'execution',-1),['chat','execution','explorer','tasks'])
assert.deepEqual(normalizeOrder(['tasks','chat'],['chat','tasks','memory']),['tasks','chat','memory'])
console.log('Navigation order: 10 normalization / move / upgrade-boundary assertions passed.')
