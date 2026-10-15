/** Fail before any model request if the target is another app or a demo build. */
export function assertLiveIdentity(html, nonce) {
  if (!/<title>\s*Metteur\s*<\/title>/i.test(html))
    throw new Error('Expected the Metteur application')
  if (!/<meta\s+name="metteur-gateway"\s+content="grpc"\s*\/?\s*>/i.test(html)) {
    throw new Error(
      'Live validation requires an explicit production grpc gateway build; mock or unlabelled builds are refused',
    )
  }
  if (nonce && !html.includes(`<meta name="metteur-live-fixture" content="${nonce}">`)) {
    throw new Error("Live validation requires this invocation's owned fixture")
  }
}
