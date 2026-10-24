// Read-only supplement: reconcile runtime errors and the host-generated blueprint.
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { readFile, readdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { isDeepStrictEqual } from 'node:util'
const [baseURL, root, output] = process.argv.slice(2)
const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: baseURL }))
const bundle = JSON.parse(await readFile(join(root, 'frozen.json'), 'utf8')),
  reports = []
const normalize = (x) =>
  JSON.parse(
    JSON.stringify(x, (key, value) => (key === 'key' && value === null ? undefined : value)),
  )
async function files(path, prefix = '') {
  const result = []
  for (const entry of await readdir(path, { withFileTypes: true })) {
    if (['.metteur', '.git', '__pycache__'].includes(entry.name)) continue
    const name = prefix + entry.name
    if (entry.isDirectory()) result.push(...(await files(join(path, entry.name), name + '/')))
    else result.push(name)
  }
  return result
}
for (const [task, mode] of bundle.order) {
  const directory = join(root, task + '-' + mode),
    workspacePath = join(directory, 'workspace')
  const driver = JSON.parse(await readFile(join(directory, 'driver.json'), 'utf8'))
  const spec = bundle.tasks.find((t) => t.id === task)
  const errors = driver.events.filter((e) => e.kind === 'error').map((e) => e.message)
  const report = {
    task,
    mode,
    runtime_completed: driver.status === 'completed' && !errors.length,
    runtime_errors: [...errors, ...(driver.error ? [driver.error] : [])],
    role_calls: {},
    unexpected_files: [],
    blueprint_matches_frozen_run: null,
  }
  await client.openWorkspace({ path: workspacePath })
  try {
    const allowed = new Set([
      ...Object.keys(spec.files),
      ...spec.allowed,
      ...(mode === 'react' ? [] : ['blueprints/task.blueprint']),
    ])
    report.unexpected_files = (await files(workspacePath)).filter((f) => !allowed.has(f))
    const audit = await client.listAuditLog({ workspacePath })
    for (const entry of audit.entries.filter((e) => e.operation === 'llm.observation.end')) {
      const d = JSON.parse(entry.detailJson),
        role = d.role || 'unknown'
      report.role_calls[role] = (report.role_calls[role] || 0) + 1
    }
    if (mode !== 'react' && driver.run) {
      const executions = await client.listExecutions({ workspacePath })
      const run = executions.executions.find((r) => r.runId === driver.run)
      if (run) {
        const snapshot = JSON.parse(run.dataJson),
          saved = JSON.parse(
            await readFile(join(workspacePath, 'blueprints/task.blueprint'), 'utf8'),
          )
        report.blueprint_matches_frozen_run = isDeepStrictEqual(
          normalize(saved),
          normalize(snapshot.view?.root),
        )
      }
    }
  } finally {
    await client.closeWorkspace({ path: workspacePath })
  }
  reports.push(report)
}
await writeFile(output, JSON.stringify(reports, null, 2))
console.log(JSON.stringify(reports))
