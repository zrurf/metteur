import type { ObjectDirective } from 'vue'

const cleanup = new WeakMap<HTMLElement, () => void>()
export const vDialogFocus: ObjectDirective<HTMLElement> = {
  mounted(element) {
    const previous = document.activeElement as HTMLElement | null
    const focusable = () => [...element.querySelectorAll<HTMLElement>('button:not(:disabled), input, textarea, select, [tabindex="0"]')]
    focusable()[0]?.focus()
    const trap = (event: KeyboardEvent) => {
      if (event.key !== 'Tab') return
      const items = focusable()
      const index = items.indexOf(document.activeElement as HTMLElement)
      event.preventDefault()
      items[(index + (event.shiftKey ? -1 : 1) + items.length) % items.length]?.focus()
    }
    element.addEventListener('keydown', trap)
    cleanup.set(element, () => { element.removeEventListener('keydown', trap); previous?.focus() })
  },
  unmounted(element) { cleanup.get(element)?.(); cleanup.delete(element) },
}
