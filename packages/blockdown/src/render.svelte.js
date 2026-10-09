// Rendered HTML per block source, filled by the host's `render`.
//
// Keyed by the block's text, so an edit re-renders only the blocks it changed
// and a diff renders each distinct block once. Frontmatter blocks never reach
// the host's renderer: markdown renderers do not know YAML, so they are drawn
// by `renderFrontmatter` (the package's key/value table by default).

import { untrack } from 'svelte';
import { frontmatterHtml } from './blocks.js';

/** Past this many entries the cache starts over rather than grow forever. */
const LIMIT = 2000;

/**
 * @param {() => (sources: string[]) => string[] | Promise<string[]>} getRender
 * @param {() => ((block: string) => string) | undefined} getFrontmatter
 * @param {(html: string) => string} [transform] applied to the host's HTML
 */
export function renderCache(getRender, getFrontmatter, transform = (html) => html) {
  /** @type {Record<string, string>} */
  let html = $state({});
  let seq = 0;

  return {
    /** The HTML for each rendered text. */
    get html() {
      return html;
    },

    /**
     * Ask for these blocks to be rendered: `[{ text, kind }]`. Call from an
     * effect; only texts not yet rendered go to the host.
     */
    want(blocks) {
      untrack(() => {
        const fm = getFrontmatter() ?? frontmatterHtml;
        const missing = [];
        for (const { text, kind } of blocks) {
          if (text in html) continue;
          if (kind === 'frontmatter') html[text] = fm(text);
          else if (!missing.includes(text)) missing.push(text);
        }
        if (missing.length === 0) return;
        const mine = ++seq;
        Promise.resolve(getRender()(missing))
          .then((out) => {
            if (Object.keys(html).length > LIMIT && mine === seq) html = {};
            missing.forEach((text, i) => {
              html[text] = transform(out[i] ?? '');
            });
          })
          .catch(() => {
            // The sources stay showing; a later change asks again.
          });
      });
    },
  };
}
