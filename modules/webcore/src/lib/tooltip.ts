/**
 * App-wide tooltips.
 *
 * The native `title` tooltip cannot be styled, inherits the OS theme, appears
 * after an unpredictable delay and is invisible to keyboard users on some
 * platforms. This module replaces it everywhere at once by *delegating* rather
 * than by rewriting every component: on hover or keyboard focus the element's
 * `title` is moved aside (so the native bubble never appears) and a styled
 * tooltip is shown instead. The attribute is restored on leave, so the DOM
 * keeps its accessible name when the tooltip is not visible.
 *
 * Elements may also carry `data-tip` directly; `data-tip="none"` opts out.
 */

/** How long the pointer must rest before the tooltip appears. */
const HOVER_DELAY_MS = 350
/** Gap between the target and the tooltip. */
const OFFSET = 8
/** Margin kept from the viewport edges when flipping or shifting. */
const VIEWPORT_MARGIN = 8

const ATTRIBUTE = 'data-tip'
/** Where the original `title` is parked while the tooltip is visible. */
const STASH = 'data-tip-title'

let tip: HTMLElement | null = null
let target: Element | null = null
let timer: ReturnType<typeof setTimeout> | undefined

/** Creates the singleton tooltip element on first use. */
function element(): HTMLElement {
  if (tip) return tip
  tip = document.createElement('div')
  tip.className = 'app-tooltip'
  tip.setAttribute('role', 'tooltip')
  tip.hidden = true
  document.body.appendChild(tip)
  return tip
}

/** The tooltip text of an element, or an empty string when it has none. */
function textOf(el: Element): string {
  if (el.getAttribute(ATTRIBUTE) === 'none') return ''
  const explicit = el.getAttribute(ATTRIBUTE)
  if (explicit) return explicit
  // The stash holds the text while the tooltip is open.
  const native = (el.getAttribute('title') ?? el.getAttribute(STASH))?.trim()
  if (native) return native
  return iconLabel(el)
}

/**
 * The label of an icon-only control, used as its tooltip.
 *
 * An IDE shell is full of buttons whose only text is an accessible label; a
 * native tooltip never showed it, and a visible one is where a user looks for
 * the name of an unfamiliar icon. Buttons that already show text are left alone
 * — a tooltip repeating the visible label is noise — and the transcript's own
 * labelled regions are not controls at all.
 */
function iconLabel(el: Element): string {
  const tag = el.tagName.toLowerCase()
  const interactive = tag === 'button' || tag === 'a' || el.getAttribute('role') === 'button'
  if (!interactive) return ''
  if ((el.textContent ?? '').trim()) return ''
  return el.getAttribute('aria-label')?.trim() ?? ''
}

/** Moves `title` out of the way so the native bubble cannot appear. */
function stashTitle(el: Element): void {
  const title = el.getAttribute('title')
  if (!title) return
  el.setAttribute(STASH, title)
  el.removeAttribute('title')
}

/** Puts the `title` back, keeping the element's accessible name intact. */
function restoreTitle(el: Element): void {
  const stashed = el.getAttribute(STASH)
  if (!stashed) return
  el.setAttribute('title', stashed)
  el.removeAttribute(STASH)
}

/** Positions the tooltip relative to `el`, flipping when there is no room. */
function place(el: Element): void {
  const node = element()
  const box = el.getBoundingClientRect()
  const size = node.getBoundingClientRect()

  let top = box.top - size.height - OFFSET
  if (top < VIEWPORT_MARGIN) top = box.bottom + OFFSET
  let left = box.left + box.width / 2 - size.width / 2
  left = Math.max(
    VIEWPORT_MARGIN,
    Math.min(left, window.innerWidth - size.width - VIEWPORT_MARGIN),
  )

  node.style.top = `${Math.round(top)}px`
  node.style.left = `${Math.round(left)}px`
}

/** Shows the tooltip for `el`. */
function show(el: Element): void {
  const text = textOf(el)
  if (!text) return
  const node = element()
  target = el
  node.textContent = text
  node.hidden = false
  // Measured after it becomes visible, otherwise the box has no size.
  place(el)
  node.id = 'app-tooltip'
  el.setAttribute('aria-describedby', 'app-tooltip')
  stashTitle(el)
}

/** Hides the tooltip and restores the target's attributes. */
function hide(): void {
  clearTimeout(timer)
  if (target) {
    restoreTitle(target)
    target.removeAttribute('aria-describedby')
    target = null
  }
  if (tip) tip.hidden = true
}

/** The element a tooltip request is about, if it should have one. */
function candidate(node: EventTarget | null): Element | null {
  if (!(node instanceof Element)) return null
  // `[${STASH}]` matters: while a tooltip is visible the element's `title` has
  // been moved aside, and re-entering it must still be recognized as the same
  // target.
  const el = node.closest(`[title], [${ATTRIBUTE}], [${STASH}], button[aria-label], a[aria-label]`)
  // A frame's title names its browsing context for assistive technology.
  // Moving it into the tooltip stash makes a hovered/focused frame unnamed.
  if (!el || el === tip || el.matches('iframe, frame')) return null
  // Never tooltip the tooltip.
  if (tip && el.contains(tip)) return null
  return el
}

/**
 * Installs the delegated listeners. Idempotent: calling it twice is a no-op.
 */
export function installTooltips(): void {
  if (document.documentElement.dataset.tooltips === 'on') return
  document.documentElement.dataset.tooltips = 'on'

  let dragging = false
  document.addEventListener('dragstart', () => { dragging = true; hide() })
  document.addEventListener('dragend', () => { dragging = false; hide() })
  document.addEventListener('drop', () => { dragging = false; hide() })

  // `pointerover` alone decides whether a tooltip should be open: it fires for
  // every element the pointer enters, including the ones that have no tooltip,
  // which is exactly where the previous one must close. A `pointerout` handler
  // cannot do this, because the element it leaves no longer carries a `title`
  // (it was stashed while the tooltip was visible).
  document.addEventListener('pointerover', (event) => {
    if (dragging) return
    const el = candidate(event.target)
    if (el === target) return
    hide()
    if (!el) return
    if (!textOf(el)) return
    timer = setTimeout(() => show(el), HOVER_DELAY_MS)
  })

  // Leaving the window entirely fires no `pointerover`.
  document.addEventListener('pointerleave', () => hide())

  document.addEventListener('focusin', (event) => {
    const el = candidate(event.target)
    if (!el) return
    // Keyboard focus is deliberate: show immediately.
    hide()
    show(el)
  })

  document.addEventListener('focusout', (event) => {
    if (candidate(event.target)) hide()
  })

  // A click or a scroll invalidates the anchor.
  document.addEventListener('pointerdown', () => hide(), true)
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') hide()
  })
  window.addEventListener('scroll', () => hide(), true)
  window.addEventListener('resize', () => hide())
}
