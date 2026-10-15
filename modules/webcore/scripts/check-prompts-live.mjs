import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { chromium, expect } from '@playwright/test'
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs'
import { randomUUID } from 'node:crypto'
import { dirname,join } from 'node:path'
import assert from 'node:assert/strict'
const ready=JSON.parse(readFileSync(process.argv[2],'utf8')), output=process.argv[3]
mkdirSync(output,{recursive:true});assert.equal(ready.baseURL,'http://127.0.0.1:18976')
const html=await(await fetch(ready.baseURL)).text();assert(html.includes(`content="${ready.nonce}"`));assert(html.includes('content="grpc"'))
assert(ready.root.startsWith('/evidence/runtime-prompts/live-'))
const workspacePath=`${ready.root}/prompt-check-${randomUUID()}`
const hostWorkspace=join(dirname(process.argv[2]),workspacePath.slice('/evidence/'.length));mkdirSync(hostWorkspace)
writeFileSync(join(hostWorkspace,'app.ts'),`export const evidence = '${ready.nonce}'\n`)
writeFileSync(`${output}/tested-workspace.json`,JSON.stringify({...ready,workspace:workspacePath},null,2))
const client=createClient(Daemon,createGrpcWebTransport({baseUrl:ready.baseURL}))
await client.openWorkspace({path:workspacePath})
const call=async(method,payload,expected=0)=>{const result=await client[method]({schemaVersion:1,requestId:randomUUID(),workspacePath,expectedRevision:BigInt(expected),payloadJson:JSON.stringify(payload)});if(result.errorJson)throw new Error(result.errorJson);return JSON.parse(result.payloadJson)}
const save=(scope,id,body,revision=0)=>call('writePromptResource',{scope,draft:{id,name:'Live review',tags:[],body}},revision)
const project=await save('project',randomUUID(),'PROJECT_SELECTED_v1'),personal=await save('personal',randomUUID(),'PERSONAL_SELECTED_v1')
assert.notEqual(project.reference.scope.scope,personal.reference.scope.scope)
const checks=[],graphs={}
// Exercise actual saved blueprint admission and both resource scopes, without a model.
for(const entry of [project,personal]){
 const graph=await client.compileDsl({workspacePath,source:'entry start: Start\np: PromptResource\nmerge: ContextMerge(Context <- start.Context, Text <- p.Text, Role = "system")\ncall: CallLLM(Context <- merge.Result, provider = "mock", mock_text = "SNAPSHOT_OK")\nend: End\nstart -> call\ncall -> end'})
 const resource=graph.nodes.find(node=>node.kind==='PromptResource')
 const snapshot=await call('selectPromptResource',{reference:entry.reference,purpose:{kind:'blueprint',blueprint_id:graph.id,node_id:resource.id}})
 resource.dataJson=JSON.stringify({snapshot_id:snapshot.id})
 graph.nodes.forEach((node,index)=>{node.posX=index*240;node.posY=120})
 graphs[entry.reference.scope.scope]=graph.id
 await client.saveBlueprint({workspacePath,blueprint:graph,filePath:`${entry.reference.scope.scope}.mbp.blueprint`})
 const events=[];for await(const event of client.executeBlueprint({workspacePath,blueprintId:graph.id}))events.push(event)
 writeFileSync(`${output}/${entry.reference.scope.scope}-run.json`,JSON.stringify(events,null,2))
 assert(!events.some(event=>event.kind==='error'),JSON.stringify(events));assert(events.some(event=>event.kind==='finished'))
 checks.push(`${entry.reference.scope.scope} actual saved blueprint with frozen resource`)
}
const browser=await chromium.launch({headless:true}),page=await browser.newPage({viewport:{width:1360,height:1050}}),errors=[]
page.on('pageerror',error=>errors.push(error.message))
try{
 await page.goto(`${ready.baseURL}/?open=${encodeURIComponent(workspacePath)}`)
 await expect(page).toHaveURL(/\/work\/[^/]+\/explorer$/)
 await page.getByText('project.mbp.blueprint',{exact:true}).first().click()
 await expect(page.getByRole('button',{name:'Save',exact:true})).toBeVisible()
 await page.getByRole('button',{name:'Prompt resources',exact:true}).first().click()
 await expect(page.getByRole('heading',{name:/Live review/})).toBeVisible()
 await page.getByRole('heading',{name:/Live review/}).dragTo(page.getByLabel('Chat drop target',{exact:true}))
 await expect(page.getByLabel('Chat drop target',{exact:true}).locator('.pv-chip')).toHaveCount(1)
 await page.getByRole('heading',{name:/Live review/}).dragTo(page.getByLabel('Blueprint drop target',{exact:true}))
 await expect(page.getByLabel('Blueprint drop target',{exact:true}).locator('.pv-chip')).toHaveCount(1)
 await page.getByRole('button',{name:'Personal',exact:true}).click()
 await page.locator('.pv-row').filter({hasText:personal.reference.id}).click()
 await page.getByRole('button',{name:'Add to Chat',exact:true}).click()
 await page.getByRole('button',{name:'Add to blueprint',exact:true}).click()
 await expect(page.getByLabel('Chat drop target',{exact:true}).locator('.pv-chip')).toHaveCount(2)
 await expect(page.getByLabel('Blueprint drop target',{exact:true}).locator('.pv-chip')).toHaveCount(2)
 // Actual revision change must block a send; refresh is an explicit user action.
 await save('personal',personal.reference.id,'PERSONAL_SELECTED_v2',1)
 await page.getByRole('button',{name:'Send message',exact:true}).click()
 await expect(page.getByRole('status')).toContainText('changed or is missing')
 assert.equal((await client.listChatSessions({workspacePath})).sessions.length,0)
 await page.getByRole('button',{name:'Refresh selection',exact:true}).click()
 await expect(page.getByLabel('Chat drop target',{exact:true})).toContainText('r2')
 await page.getByRole('button',{name:'Send message',exact:true}).click()
 await expect(page.getByText('Sent history · immutable user content')).toBeVisible({timeout:30000})
 const sessions=(await client.listChatSessions({workspacePath})).sessions;assert.equal(sessions.length,1)
 const sessionId=sessions[0].id||sessions[0].sessionId
 const history=await client.getChatSession({workspacePath,sessionId})
 const messages=JSON.parse(history.historyJson),chosen=messages.find(message=>message.role==='user'&&message.content.includes('PROJECT_SELECTED_v1'))
 assert(chosen);assert(chosen.content.includes('PERSONAL_SELECTED_v2'));assert(!chosen.content.includes('PERSONAL_SELECTED_v1'))
 checks.push('same-name scopes via drag and keyboard buttons','changed selection blocks send','explicit refresh','actual Chat user content and persistent snapshots')
 await call('deletePromptResource',{scope:'personal',id:personal.reference.id},2)
 const restored=await client.getChatSession({workspacePath,sessionId});assert.equal(restored.historyJson,history.historyJson)
 await page.getByRole('button',{name:'Reload',exact:true}).click()
 await expect(page.getByText('Loading resources…',{exact:true})).toBeHidden()
 await expect(page.locator('.pv-row').filter({hasText:personal.reference.id})).toHaveCount(0)
 checks.push('source deletion leaves sent history unchanged')
 await page.screenshot({path:`${output}/prompts-live-history.png`,fullPage:true})
 // Consume queued selections into the real editor and save their stable identities.
 await page.getByText('project.mbp.blueprint',{exact:true}).first().click()
 await expect(page.getByRole('button',{name:'Save',exact:true})).toBeVisible()
 await page.getByRole('button',{name:'Save',exact:true}).click()
 await expect(page.getByTestId('blueprint-save-error')).toHaveCount(0)
 await expect.poll(async()=>{const result=await client.loadBlueprint({workspacePath,blueprintId:graphs.project});return result.nodes.filter(node=>node.kind==='PromptResource').length}).toBe(3)
 const loaded=await client.loadBlueprint({workspacePath,blueprintId:graphs.project})
 assert(loaded.nodes.filter(node=>node.kind==='PromptResource').every(node=>JSON.parse(node.dataJson).snapshot_id))
 await assert.rejects(async()=>{for await(const event of client.executeBlueprint({workspacePath,blueprintId:graphs.project})){void event}},/unavailable|changed|missing/)
 checks.push('both scopes create saved canvas nodes and ContextMerge edges','missing or changed source blocks next run')
 writeFileSync(`${output}/blueprint.json`,JSON.stringify(loaded,null,2))
 await page.screenshot({path:`${output}/prompts-live-canvas.png`,fullPage:true})
 assert.deepEqual(errors,[])
 writeFileSync(`${output}/result.json`,JSON.stringify({passed:true,preview:'PV-02/r1 approved',workspace:workspacePath,checks,paidCalls:0},null,2))
 console.log('Production prompt resources / Chat / blueprint targeted check: PASS')
}catch(error){await page.screenshot({path:`${output}/failure.png`,fullPage:true});writeFileSync(`${output}/failure.txt`,String(error)+'\n'+await page.locator('body').innerText());throw error}finally{await browser.close()}
