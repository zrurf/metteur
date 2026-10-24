// Prepare a fresh human-review sample; this script never starts a model or run.
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { randomUUID } from 'node:crypto'
import assert from 'node:assert/strict'

const [readyPath, destination] = process.argv.slice(2)
const ready = JSON.parse(await readFile(readyPath, 'utf8')),
  root = resolve(destination),
  workspace = join(root, 'workspace')
assert.equal(new URL(ready.baseURL).hostname, '127.0.0.1')
await mkdir(root)
await mkdir(workspace)
const files = {
  'app.ts': `export const evidence = '${ready.nonce}'\n`,
  'shared.py': 'def normalize(value):\n    return value.strip()\n',
  'alpha.py': 'from shared import normalize\ndef alpha(value):\n    return normalize(value)\n',
  'beta.py': 'from shared import normalize\ndef beta(value):\n    return normalize(value)\n',
  'test_shared.py':
    'import unittest, time\nfrom alpha import alpha\nfrom beta import beta\nclass SharedTest(unittest.TestCase):\n    def test_both_callers(self):\n        time.sleep(8)\n        self.assertEqual(alpha(" A "), "a")\n        self.assertEqual(beta(" B "), "b")\n',
  'feature.py': 'def pick(values, query, limit=None):\n    return []\n',
  'test_stage1.py':
    'import unittest\nfrom feature import pick\nclass StageOne(unittest.TestCase):\n    def test_filter(self):\n        self.assertEqual(pick(["Alpha", "beta", "ALP"], "al"), ["Alpha", "ALP"])\n',
  'test_stage2.py':
    'import unittest\nfrom feature import pick\nclass StageTwo(unittest.TestCase):\n    def test_limit(self):\n        self.assertEqual(pick(["a", "aa"], "a", 1), ["a"])\n        self.assertEqual(pick(["a"], "a", 0), [])\n        with self.assertRaises(ValueError):\n            pick(["a"], "a", -1)\n',
}
for (const [path, content] of Object.entries(files)) await writeFile(join(workspace, path), content)
const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL }))
await client.openWorkspace({ path: workspace })
const resources = []
for (const scope of ['personal', 'project']) {
  const response = await client.writePromptResource({
    schemaVersion: 1,
    requestId: randomUUID(),
    workspacePath: workspace,
    payloadJson: JSON.stringify({
      scope,
      draft: {
        id: randomUUID(),
        name: 'Trial normalization rules',
        tags: ['daily-workflow-trial'],
        body:
          scope === 'personal'
            ? 'DW_PERSONAL_NORMALIZATION: explain changes and preserve both public callers.'
            : 'DW_PROJECT_NORMALIZATION: keep caller signatures and validate with actual unittest.',
      },
    }),
  })
  assert.equal(response.errorJson, '')
  resources.push(JSON.parse(response.payloadJson).reference)
}
for (const [id, name, path, command] of [
  [
    '5726e3bf-35c8-5e17-8942-78756af9c2cc',
    'Story A normalization checks',
    'story-a',
    'py -3.12 -m unittest test_shared.py',
  ],
  [
    'b0548572-8998-4d1b-9f71-27d5946b0001',
    'Story B filtering checks',
    'story-b1',
    'py -3.12 -m unittest test_stage1.py',
  ],
  [
    'b0548572-8998-4d1b-9f71-27d5946b0002',
    'Story B all checks',
    'story-b2',
    'py -3.12 -m unittest test_stage1.py test_stage2.py',
  ],
]) {
  const graph = await client.compileDsl({
    workspacePath: workspace,
    source: `entry start: Start\nrun: ExecuteCommand(command = ${JSON.stringify(command)})\nverify: Validator(Actual <- run.Result, mode = "regex", Expected = 0)\nend: End\nstart -> run\nrun -> verify\nverify -> end`,
  })
  graph.id = id
  graph.name = name
  graph.nodes.forEach((node, index) => {
    node.posX = index * 320
    node.posY = 180
  })
  const check = graph.nodes.find((node) => node.kind === 'Validator')
  check.dataJson = JSON.stringify({
    ...JSON.parse(check.dataJson),
    regex: '"exit_code"\\s*:\\s*0(?:,|})',
  })
  await client.saveBlueprint({
    workspacePath: workspace,
    blueprint: graph,
    filePath: `${path}.blueprint`,
  })
}
const proof = await (await fetch(`${ready.providerUrl}/fixture`)).json()
const result = {
  candidate: ready.nativeCommit ?? null,
  frontendCandidate: ready.frontendCommit ?? null,
  readyPath,
  workspace,
  url: `${ready.baseURL}/?open=${encodeURIComponent(workspace)}`,
  provider: 'owned local scripted fixture; no paid model',
  promptResources: resources,
  goals: {
    storyA: 'DW_STORY_A: 修复两个调用方共用的归一化，实际验证并返回这里。',
    storyB: 'DW_STORY_B_STAGED: 先实现筛选并验证，再加入limit并验证两阶段，保持同一任务和预算。',
    supervision: 'DW03_SUPERVISION_MESSAGE: 根据宿主记录报告进度，不要修改文件。',
  },
  providerCallsAfterSeeding: proof.calls,
  seededUtc: new Date().toISOString(),
}
await writeFile(join(root, 'trial.json'), JSON.stringify(result, null, 2))
console.log(JSON.stringify(result))
