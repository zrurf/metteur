import type { UsageSummary } from './types'

/** Aggregate one RPC snapshot; never accumulate percentages or successive polls. */
export function executionUsage(summary: UsageSummary | null) {
  const models = summary?.models ?? []
  const valid = (value: number) => Number.isSafeInteger(value) && value >= 0
  const complete = models.length > 0 && models.every(m => m.calls > 0 && m.tokensComplete === true && valid(m.inputTokens) && valid(m.outputTokens))
  const input = models.reduce((sum, m) => sum + m.inputTokens, 0)
  const output = models.reduce((sum, m) => sum + m.outputTokens, 0)
  const cached = models.reduce((sum, m) => sum + m.cachedInputTokens, 0)
  const cacheComplete = complete && valid(input) && input > 0 && valid(cached) && models.every(m => m.cacheComplete === true && valid(m.cachedInputTokens) && m.cachedInputTokens <= m.inputTokens)
  const costComplete = complete && models.every(m => m.costComplete === true && valid(m.costMicros)) && valid(summary!.totalCostMicros)
  return { input: complete && valid(input) ? input : null, output: complete && valid(output) ? output : null,
    cacheHitRate: cacheComplete ? cached / input : null, estimatedCost: costComplete ? summary!.totalCostMicros / 1_000_000 : null }
}
