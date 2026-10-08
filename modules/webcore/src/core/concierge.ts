export interface ConciergeTurn {
  id: string
  at_ms: number
  conversation_id: string
  original_text: string
  answer: string
  state: string
  request_id: string | null
  error: string | null
}
export interface ConciergeRequest {
  request_id: string
  run_id: string
  conversation_id: string
  original_text: string
  concierge_note: string
  state: string
  source: 'concierge_forwarded'
  review_id: string | null
  proposals: Array<{ proposal_id: string; state: string; result_refs: string[] }>
  result_refs: string[]
}
export interface ConciergeState {
  run_id: string
  conversation_id: string
  available: boolean
  read_only: boolean
  reason: string
  consumer_enabled: boolean
  reports?: import('./oversight').OversightReview[]
  messages: ConciergeTurn[]
  requests: ConciergeRequest[]
}
export interface ConciergeEvent { runId: string; messageId: string; kind: string; turn?: ConciergeTurn }
