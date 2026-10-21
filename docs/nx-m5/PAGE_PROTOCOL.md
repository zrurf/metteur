# Addon page protocol v1

PV-06/r1 and PV-07/r1 provisionally approved by user message Sentinel_106b3f6af6ec819191a269986e0ac706 (2026-10-15 11:46:54 UTC). Final new-page review remains required.

- Page entries are manifest-declared self-contained UTF-8 HTML, maximum 256 KiB each / eight pages. Canonical package paths only. Assets are part of the signed immutable snapshot; page packages require a valid signature even if tool-only development packages allow unsigned loading. Existing trust-key policy remains authoritative; a self-signature without configured trust keys is integrity, not publisher verification.
- Host creates a 15-minute lease bound to package ID/version/digest, page, canonical workspace, authenticated subject, random instance and nonce. URL/route gives no authority. Limit: 64 active pages.
- Page runs in a srcdoc iframe with sandbox=allow-scripts only, no same-origin, forms, popup or top-navigation capability. Its CSP denies connections and external resources. Parent frame-src 'none' also prevents child navigation to host app or other URLs while allowing srcdoc in the supported Chromium browser. This restriction is served in HTTP and embedded in the native entry.
- Host transfers a dedicated MessageChannel to the exact frame contentWindow after load; only that port is accepted. The nonce, RPC client, workspace root and authenticated subject are never sent to the iframe.
- Message: bridge_version=1, UUID request_id, allowlisted method and strictly typed params. Request envelope max16KiB; 32 messages/second; 4096 per lease; maximum four in flight in UI. Duplicate request IDs rejected. Unknown methods/fields, paths, approval data or other ownership rejected. Declared capabilities checked on each call.
- Disable/upgrade/uninstall reconcile package identities and revoke old leases; workspace close revokes leases. UI removes invalid frames on polling/failed calls; backend rejects immediately, including before the UI notices. Page navigation invalidates the channel. No retry after uncertain side effects.
- Page operations cannot create grants, approve model execution, run shells or access personal Prompt resources.

NX-41 supports explicitly selected, host-registered local HTML snapshots and saved source-diagnostic reports. No URL fetching, rendering, console capture or model call is implied. Host Review is outside the iframe and requires exact report ID/content digest; no bridge method can invoke it.

The heartbeat checks the bound lease directly. It does not recompile/reload packages or reconnect addon services on every tick. Only discovery/open performs package admission refresh. Theme changes are forwarded over the existing channel.
