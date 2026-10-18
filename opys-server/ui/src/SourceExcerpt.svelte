<script>
  import { sourceLines } from './lib/verification.js';
  let { finding } = $props();
  const lines = $derived(sourceLines(finding));
  const insertion = $derived(lines.some((line) => line.point));
</script>

<div class="source-excerpt" aria-label="Markdown source with validation highlight">
  {#if insertion}<p class="source-note">Insert the missing content at the highlighted position.</p>{/if}
  <pre>{#each lines as line}<span class="source-line" class:selected={line.selected}><span class="line-number" aria-hidden="true">{line.number}</span><code>{line.before}{#if line.selected}<mark class:insertion={line.point}>{line.marked || ' '}</mark>{/if}{line.after}</code></span>{/each}</pre>
</div>

<style>
  .source-excerpt { margin:0.6rem 0; border:1px solid var(--border); border-radius:5px; background:var(--bg); }
  .source-note { margin:0; padding:0.5rem 0.65rem; color:var(--muted); font-size:0.78rem; border-bottom:1px solid var(--border); }
  pre { margin:0; padding:0.45rem 0; overflow:auto; max-height:20rem; font-size:0.78rem; line-height:1.7; }
  .source-line { display:flex; min-width:max-content; }
  .source-line.selected { background:color-mix(in srgb,var(--bad) 6%,transparent); }
  .line-number { width:4.5ch; flex:none; padding-right:1ch; text-align:right; color:var(--muted); user-select:none; }
  code { display:block; padding-right:1rem; font:inherit; }
  mark { color:inherit; background:color-mix(in srgb,var(--bad) 23%,transparent); text-decoration:underline wavy var(--bad); text-underline-offset:3px; }
  mark.insertion { border-left:2px solid var(--bad); text-decoration:none; }
</style>
