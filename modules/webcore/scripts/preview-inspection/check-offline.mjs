// One targeted renderer boundary test; local listener is a leak detector only.
import { createServer } from 'node:http'
import { once } from 'node:events'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import assert from 'node:assert/strict'
const out=process.argv[2];if(!out)throw Error('Provide output JSON path')
let hits=0;const server=createServer((_,res)=>{hits++;res.end('should not be reached')});server.listen(0,'127.0.0.1');await once(server,'listening');
try{
 const url=`http://127.0.0.1:${server.address().port}/blocked`
 const html=`<!doctype html><title>Boundary fixture</title><meta name="viewport" content="width=device-width"><div style="width:2000px">Overflow fixture</div><img src="${url}"><iframe src="${url}"></iframe><script>console.info('boundary-fixture-ran');fetch('${url}').catch(()=>{});window.open('${url}')</script>`
 const digest=createHash('sha256').update(html).digest('hex')
 const child=spawn(process.execPath,[fileURLToPath(new URL('./render.mjs',import.meta.url))],{env:process.env,stdio:['pipe','pipe','pipe']});let output='',error='';child.stdout.on('data',c=>output+=c);child.stderr.on('data',c=>error+=c);child.stdin.end(JSON.stringify({schema_version:1,html,digest}));const [code]=await once(child,'exit');assert.equal(code,0,error)
 const r=JSON.parse(output);assert.equal(r.digest,digest);assert.equal(r.browser_checks,'completed');assert.equal(hits,0);assert.ok(r.metrics.scrollWidth>r.metrics.viewportWidth);assert.ok(r.console.entries.some(e=>e.message==='boundary-fixture-ran'));assert.ok(r.console.entries.some(e=>e.level==='error'));assert.ok(r.screenshot.base64.length>100)
 await writeFile(out,JSON.stringify({status:'passed',networkHits:hits,overflowDetected:true,consoleCaptured:true,screenshotBytes:Buffer.from(r.screenshot.base64,'base64').length,digest,scope:'single controlled hostile HTML fixture; not complete browser security certification'},null,2))
}finally{server.close()}
