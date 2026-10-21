// Bundle the existing RPC dependencies; the shipped launcher needs Node only.
import { createRequire } from 'node:module'
import { pathToFileURL, fileURLToPath } from 'node:url'
import { resolve, dirname, join, isAbsolute } from 'node:path'
import { mkdir, cp, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
const web = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const destination = process.argv[2]
if (!destination || !isAbsolute(destination)) throw Error('Pass an absolute output directory')
const output = resolve(destination)
const require = createRequire(import.meta.url)
const viteRequire = createRequire(require.resolve('vite/package.json'))
const { rolldown } = await import(pathToFileURL(viteRequire.resolve('rolldown')))
await mkdir(join(output, 'scripts'), { recursive: true })
const bundle = await rolldown({ input: join(web, 'scripts/run-nx-local.mjs'), platform: 'node', external: /^node:/ })
try {
  await bundle.write({ file: join(output, 'scripts/run-nx-local.mjs'), format: 'esm', codeSplitting: false })
} finally { await bundle.close() }
await cp(join(web, 'scripts/fixtures'), join(output, 'scripts/fixtures'), { recursive: true })
const notices = []
for (const name of ['@connectrpc/connect', '@connectrpc/connect-web', '@bufbuild/protobuf']) {
  let folder = dirname(require.resolve(name))
  while (folder !== dirname(folder)) {
    try {
      const metadata = JSON.parse(await readFile(join(folder, 'package.json'), 'utf8'))
      if (metadata.name === name) {
        const source = name.startsWith('@connectrpc/') ? 'connect-es' : 'protobuf-es'
        notices.push(`${name}@${metadata.version}\nSPDX: ${metadata.license}\nSource: ${metadata.repository.url}\n${await readFile(join(web, `scripts/portable-licenses/${source}.txt`), 'utf8')}`)
        break
      }
    } catch { /* Walk up from the resolved ESM entry to its package root. */ }
    folder = dirname(folder)
  }
  if (notices.length < ['@connectrpc/connect', '@connectrpc/connect-web', '@bufbuild/protobuf'].indexOf(name) + 1) throw Error(`Missing license: ${name}`)
}
notices.push(`Protocol Buffers generated/runtime portions: BSD-3-Clause\nSource: https://github.com/protocolbuffers/protobuf/blob/main/LICENSE\n${await readFile(join(web, 'scripts/portable-licenses/protobuf-google-bsd.txt'), 'utf8')}`)
await writeFile(join(output, 'THIRD-PARTY-NOTICES.txt'), notices.join('\n\n'), 'utf8')
const code = await readFile(join(output, 'scripts/run-nx-local.mjs'))
console.log(JSON.stringify({ output, bundledBytes: code.length, sha256: createHash('sha256').update(code).digest('hex'), dependencies: 3, licenseTexts: notices.length }))
