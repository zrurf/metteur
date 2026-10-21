/**
 * Markdown rendering for assistant messages and document previews.
 *
 * The model's answers are markdown; showing them as plain text hides code
 * blocks, lists and tables. Rendering is deliberately conservative:
 * `html: false` means raw HTML in an answer is never interpreted, and the
 * produced HTML is sanitized once more before it reaches the DOM.
 *
 * Syntax highlighting uses Shiki (the grammar set VS Code uses) with a small
 * preloaded grammar set and the JavaScript regex engine, so no WASM is needed.
 * The highlighter loads lazily: the first render shows plain code and is
 * upgraded in place once the grammars are ready.
 */

import DOMPurify from 'dompurify'
import MarkdownIt from 'markdown-it'

/**
 * Grammar loaders, keyed by the Shiki language id.
 *
 * Every specifier is a static string: a template-literal import cannot be
 * analysed by the bundler, which silently turns the whole feature off in dev
 * *and* in the production build.
 */
const GRAMMAR_LOADERS: Record<string, () => Promise<{ default: unknown }>> = {
  typescript: () => import('shiki/langs/typescript.mjs'),
  tsx: () => import('shiki/langs/tsx.mjs'),
  javascript: () => import('shiki/langs/javascript.mjs'),
  jsx: () => import('shiki/langs/jsx.mjs'),
  json: () => import('shiki/langs/json.mjs'),
  toml: () => import('shiki/langs/toml.mjs'),
  markdown: () => import('shiki/langs/markdown.mjs'),
  html: () => import('shiki/langs/html.mjs'),
  css: () => import('shiki/langs/css.mjs'),
  scss: () => import('shiki/langs/scss.mjs'),
  less: () => import('shiki/langs/less.mjs'),
  python: () => import('shiki/langs/python.mjs'),
  rust: () => import('shiki/langs/rust.mjs'),
  go: () => import('shiki/langs/go.mjs'),
  shellscript: () => import('shiki/langs/shellscript.mjs'),
  powershell: () => import('shiki/langs/powershell.mjs'),
  sql: () => import('shiki/langs/sql.mjs'),
  yaml: () => import('shiki/langs/yaml.mjs'),
  xml: () => import('shiki/langs/xml.mjs'),
  diff: () => import('shiki/langs/diff.mjs'),
  c: () => import('shiki/langs/c.mjs'),
  cpp: () => import('shiki/langs/cpp.mjs'),
  java: () => import('shiki/langs/java.mjs'),
  ruby: () => import('shiki/langs/ruby.mjs'),
  php: () => import('shiki/langs/php.mjs'),
}

/** Fence tag -> Shiki language id. */
const GRAMMAR_BY_TAG: Record<string, string> = {
  ts: 'typescript',
  typescript: 'typescript',
  tsx: 'tsx',
  js: 'javascript',
  javascript: 'javascript',
  jsx: 'jsx',
  mjs: 'javascript',
  cjs: 'javascript',
  json: 'json',
  toml: 'toml',
  md: 'markdown',
  markdown: 'markdown',
  html: 'html',
  css: 'css',
  scss: 'scss',
  less: 'less',
  py: 'python',
  python: 'python',
  rs: 'rust',
  rust: 'rust',
  go: 'go',
  sh: 'shellscript',
  bash: 'shellscript',
  shell: 'shellscript',
  zsh: 'shellscript',
  powershell: 'powershell',
  ps1: 'powershell',
  sql: 'sql',
  yaml: 'yaml',
  yml: 'yaml',
  xml: 'xml',
  diff: 'diff',
  c: 'c',
  cpp: 'cpp',
  h: 'c',
  java: 'java',
  rb: 'ruby',
  php: 'php',
}

/** Grammars loaded up front; everything else loads when first used. */
const PRELOADED_GRAMMARS = ['typescript', 'javascript', 'json', 'markdown', 'shellscript', 'diff']

/** Themes matching the app's palette (Shiki bundles both). */
const THEME_LIGHT = 'github-light'
const THEME_DARK = 'github-dark'

interface Highlighter {
  codeToHtml: (code: string, options: { lang: string; themes: { light: string; dark: string }; defaultColor: false }) => string
  getLoadedLanguages: () => string[]
  loadLanguage: (...grammars: unknown[]) => Promise<void>
}

let highlighter: Highlighter | null = null
let loading: Promise<void> | null = null
/** Bumped whenever highlighting becomes available so views can re-render. */
let renderRevision = 0
const listeners = new Set<() => void>()
/** Grammars whose lazy load is already in flight. */
const pendingGrammars = new Set<string>()

/** Whether syntax highlighting is available. */
export function highlightReady(): boolean {
  return highlighter !== null
}

/** Current revision, so a view can invalidate its cached HTML. */
export function highlightRevision(): number {
  return renderRevision
}

/** Subscribe to highlighter readiness (returns an unsubscribe function). */
export function onHighlightReady(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Notifies views that rendered markdown should be rebuilt. */
function bumpRevision(): void {
  renderRevision += 1
  listeners.forEach((listener) => listener())
}

/** Loads Shiki on first use; failures degrade to unhighlighted code. */
async function ensureHighlighter(): Promise<void> {
  if (highlighter || loading) return loading ?? Promise.resolve()
  loading = (async () => {
    try {
      const [{ createHighlighterCore }, { createJavaScriptRegexEngine }, themesModule] =
        await Promise.all([
          import('shiki/core'),
          import('shiki/engine/javascript'),
          import('shiki/themes'),
        ])
      const themes = themesModule.bundledThemes as Record<string, unknown>
      const grammars = await Promise.all(
        PRELOADED_GRAMMARS.map((grammar) => GRAMMAR_LOADERS[grammar]?.()).filter(Boolean),
      )
      highlighter = (await createHighlighterCore({
        themes: [themes[THEME_LIGHT], themes[THEME_DARK]] as never[],
        langs: grammars.map((module) => module.default) as never[],
        engine: createJavaScriptRegexEngine(),
      })) as unknown as Highlighter
      bumpRevision()
    } catch {
      // Highlighting is a nicety: without it the code still renders as text.
      highlighter = null
    }
  })()
  return loading
}

/** Kick off loading without waiting (called when a chat view mounts). */
export function warmHighlighter(): void {
  void ensureHighlighter()
}

/** Loads one grammar on demand, then re-renders whatever used it. */
function requestGrammar(grammar: string): void {
  const loader = GRAMMAR_LOADERS[grammar]
  if (!loader || pendingGrammars.has(grammar)) return
  pendingGrammars.add(grammar)
  void loader()
    .then((module) => highlighter?.loadLanguage(module.default))
    .then(() => bumpRevision())
    .catch(() => undefined)
}

/** Escapes text for the plain fallback fence. */
function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

/** Resolves the Shiki grammar for a fence tag, or `null` when unknown. */
function grammarFor(tag: string): string | null {
  const normalized = tag.trim().toLowerCase().split(/[\s:]/)[0] ?? ''
  return GRAMMAR_BY_TAG[normalized] ?? null
}

/** Fallback fence: escaped text in a labelled pre block. */
function plainFence(code: string, tag: string): string {
  const label = tag ? `<span class="md-lang">${escapeHtml(tag)}</span>` : ''
  return `<div class="md-code">${label}<pre class="md-pre"><code>${escapeHtml(code)}</code></pre></div>`
}

/** Renders one code fence, highlighted when its grammar is ready. */
function renderFence(code: string, tag: string): string {
  const grammar = grammarFor(tag)
  if (!grammar || !highlighter) return plainFence(code, tag)
  if (!highlighter.getLoadedLanguages().includes(grammar)) {
    requestGrammar(grammar)
    return plainFence(code, tag)
  }
  try {
    const html = highlighter.codeToHtml(code, {
      lang: grammar,
      // Both palettes travel with the cached HTML; CSS selects the active one.
      // No stale inline dark background survives a theme switch.
      themes: { light: THEME_LIGHT, dark: THEME_DARK },
      defaultColor: false,
    })
    const label = tag ? `<span class="md-lang">${escapeHtml(tag)}</span>` : ''
    return `<div class="md-code">${label}${html}</div>`
  } catch {
    return plainFence(code, tag)
  }
}

type MarkdownRenderer = ReturnType<typeof MarkdownIt>

function createRenderer(highlight: boolean): MarkdownRenderer {
  const md = new MarkdownIt({
    html: false,
    linkify: true,
    breaks: false,
    typographer: false,
  })
  md.renderer.rules.fence = (tokens, index) => {
    const token = tokens[index]
    if (!highlight) return plainFence(token.content, token.info ?? '')
    return renderFence(token.content, token.info ?? '')
  }
  // Links open in a new tab; the app is a local shell and never navigates away.
  const defaultLinkOpen =
    md.renderer.rules.link_open ??
    ((tokens, index, options, _env, self) => self.renderToken(tokens, index, options))
  md.renderer.rules.link_open = (tokens, index, options, env, self) => {
    tokens[index].attrSet('target', '_blank')
    tokens[index].attrSet('rel', 'noreferrer noopener')
    return defaultLinkOpen(tokens, index, options, env, self)
  }
  return md
}

let renderer: MarkdownRenderer | null = null
/** Renderer used for messages that are still streaming (no highlighting). */
let plainRenderer: MarkdownRenderer | null = null

/**
 * Renders markdown to sanitized HTML.
 *
 * `highlight: false` keeps code fences as plain escaped text: a message that is
 * still streaming would otherwise re-run the highlighter on every delta.
 * Highlighting is the expensive part, not parsing.
 */
export function renderMarkdown(source: string, options?: { highlight?: boolean }): string {
  if (!source.trim()) return ''
  const highlight = options?.highlight ?? true
  if (highlight) renderer ??= createRenderer(true)
  else plainRenderer ??= createRenderer(false)
  const active = highlight ? renderer : plainRenderer
  const html = active!.render(source)
  // Shiki emits <span style="color:…">; the target/rel attributes come from
  // the link rule above and are the only additions the sanitizer allows.
  return DOMPurify.sanitize(html, {
    ADD_ATTR: ['target', 'rel'],
  })
}
