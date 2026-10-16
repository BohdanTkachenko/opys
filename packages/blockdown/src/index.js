// svelte-blockdown: a block-level markdown editor and diff for Svelte 5.
export { default as Blockdown } from './Blockdown.svelte';
export { default as BlockdownDiff } from './BlockdownDiff.svelte';
export { segment, splice, insertBlock, toggleTask, frontmatterHtml, escapeHtml } from './blocks.js';
export { diffBlocks, diffWords } from './diff.js';
export { locate } from './selection.js';
