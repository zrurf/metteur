import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { chromium, expect } from '@playwright/test'
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs'
import { randomUUID } from 'node:crypto'
import assert from 'node:assert/strict'
const ready = JSON.parse(readFileSync(process.argv[2], 'utf8'))
const output = process.argv[3]; mkdirSync(output, { recursive:true })
assert.equal(ready.baseURL, 'http://127.0.0.1:18975')
const client = createClient(Daemon, createGrpcWebTransport({ baseUrl:ready.baseURL }))
const html=await (await fetch(ready.baseURL)).text()
assert(html.includes(`content="${ready.nonce}"`)); assert(html.includes('content="grpc"'))
await client.openWorkspace({ path:ready.workspace })
const id=randomUUID()
const call=async(method,payload,expected=0) => {
  const result=await client[method]({schemaVersion:1,requestId:randomUUID(),workspacePath:ready.workspace,expectedRevision:BigInt(expected),payloadJson:JSON.stringify(payload)})
  if(result.errorJson) throw new Error(result.errorJson)
  return JSON.parse(result.payloadJson)
}
await call('writeProjectNote',{id,title:'Live memory verification',body:'A saved note with a real file binding.',files:['app.ts']})
const browser=await chromium.launch({headless:true})
const page=await browser.newPage({viewport:{width:1360,height:1050}})
const errors=[];page.on('pageerror',error=>errors.push(error.message))
try {
  await page.goto(`${ready.baseURL}/?open=${encodeURIComponent(ready.workspace)}`)
  await expect(page).toHaveURL(/\/work\/[^/]+\/explorer$/)
  await page.getByRole('button',{name:'Project memory',exact:true}).first().click()
  await expect(page.getByRole('heading',{name:'Live memory verification'})).toBeVisible()
  await page.getByRole('button',{name:'Pin as convention'}).click()
  await page.getByRole('button',{name:'Confirm convention'}).click()
  await expect(page.getByRole('status')).toContainText('confirmed as a project convention')
  let entry=await call('queryProjectMemory',{operation:'read',id})
  assert.equal(entry.content.origin,'user_convention'); assert.equal(entry.reference.revision,2)
  await page.reload()
  await expect(page.getByRole('heading',{name:'Live memory verification'})).toBeVisible()
  await page.getByRole('button',{name:'Edit note'}).click()
  await page.getByLabel('Note body').fill('Changed through the production UI.')
  await page.getByRole('button',{name:'Save new revision'}).click()
  await expect(page.getByRole('status')).toContainText('no longer applies')
  entry=await call('queryProjectMemory',{operation:'read',id})
  assert.equal(entry.content.body,'Changed through the production UI.');assert.equal(entry.content.confirmation,null)
  await call('writeProjectNote',{id,title:'Live memory verification',body:'A concurrent edit',files:['app.ts']},3)
  await page.getByRole('button',{name:'Edit note'}).click()
  await page.getByLabel('Note body').fill('This stale draft must not overwrite the concurrent edit.')
  await page.getByRole('button',{name:'Save new revision'}).click()
  await expect(page.getByRole('status')).toContainText('revision_conflict')
  await expect(page.getByLabel('Note body')).toHaveValue('This stale draft must not overwrite the concurrent edit.')
  assert.equal((await call('queryProjectMemory',{operation:'read',id})).content.body,'A concurrent edit')
  await page.screenshot({path:`${output}/memory-live-conflict.png`,fullPage:true})
  assert.deepEqual(errors,[])
  writeFileSync(`${output}/result.json`,JSON.stringify({passed:true,preview:'PV-01/r1',workspace:ready.workspace,note:id,checks:['production gateway identity','real RPC persistence','exact revision confirmation','reload','edit revokes confirmation','conflict preserves draft'],paidCalls:0},null,2))
  console.log('Production Web → grpc-web → daemon memory check: PASS')
} finally { await browser.close() }
