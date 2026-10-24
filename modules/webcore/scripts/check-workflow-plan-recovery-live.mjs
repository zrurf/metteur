import {chromium,expect} from '@playwright/test'
import {createClient} from '@connectrpc/connect'
import {createGrpcWebTransport} from '@connectrpc/connect-web'
import {Daemon} from '../src/gen/metteur_pb.ts'
import {readFile,writeFile} from 'node:fs/promises'
const [readyPath,output]=process.argv.slice(2),ready=JSON.parse(await readFile(readyPath,'utf8'))
const rpc=createClient(Daemon,createGrpcWebTransport({baseUrl:ready.baseURL}))
const browser=await chromium.launch(),page=await browser.newPage({viewport:{width:1440,height:1000}})
const report={checks:[],errors:[]},writes=[],marker=process.argv[4]||'Partial plan '+Date.now()
page.on('pageerror',e=>report.errors.push(String(e)))
page.on('request',r=>{if(/ExecuteBlueprint$|SendChat$|SaveBlueprint$/.test(r.url()))writes.push(r.url())})
async function recovery(action){const r=await rpc.recovery({workspacePath:ready.workspace,schemaVersion:1,requestId:crypto.randomUUID(),payloadJson:JSON.stringify({scope:'project',action})});expect(r.errorJson).toBe('');return JSON.parse(r.payloadJson)}
async function openPlan(){await page.getByRole('navigation',{name:'Main navigation'}).getByRole('button',{name:'Tasks',exact:true}).click();await page.getByRole('button',{name:'Start from template',exact:true}).click()}
try{
 await page.goto(ready.baseURL+'/?open='+encodeURIComponent(ready.workspace));await openPlan()
 if(!process.argv[4]){
 await page.getByRole('button',{name:/Add a small feature/}).click()
 await page.getByLabel('Plan name',{exact:true}).fill(marker)
 await page.getByLabel('Expected outcome',{exact:true}).fill('Retain an incomplete requirement after refresh')
 await expect(page.getByText('Plan backed up by the host',{exact:true}).first()).toBeVisible()
 }
 const list=await recovery({operation:'list',after:null})
 let entity=''
 for(const row of list.items.filter(r=>r.kind==='workflow'&&!r.discarded)){
   const copy=await recovery({operation:'read',id:row.id})
   if(JSON.parse(copy.record.input.raw).input.name===marker){entity=row.entity;break}
 }
 expect(entity).not.toBe('')
 await page.reload();await openPlan()
 await page.getByRole('button',{name:'Recovery copies',exact:true}).click()
 await page.getByRole('dialog',{name:'Recovery Copies'}).locator('article').filter({hasText:entity}).first().getByRole('button',{name:'Review Copy',exact:true}).click()
 await page.getByRole('button',{name:'Use This Copy',exact:true}).click()
 await page.getByRole('button',{name:'2 · Goal & checks',exact:true}).click()
 await expect(page.getByLabel('Plan name',{exact:true})).toHaveValue(marker)
 await expect(page.getByLabel('Expected outcome',{exact:true})).toHaveValue('Retain an incomplete requirement after refresh')
 await expect(page.getByLabel('Files in scope',{exact:true})).toHaveValue('')
 expect(writes).toHaveLength(0)
 report.checks.push('An incomplete plan survives host ACK, page refresh and explicit Recovery copy restoration without saving a blueprint or running')
}catch(e){report.failure=String(e);await writeFile(output+'.page.txt',await page.locator('body').innerText())}
await page.screenshot({path:output+'.png'});await writeFile(output,JSON.stringify(report,null,2));await browser.close();console.log(JSON.stringify(report));if(report.failure||report.errors.length)process.exitCode=1
