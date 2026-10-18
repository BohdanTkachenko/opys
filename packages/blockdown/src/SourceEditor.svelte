<script module>
  // Source fields mount only when Blockdown opens an edit, after hydration.
  let nextNoteId = 0;
</script>

<script>
  import { diagnosticPieces } from './diagnostics.js';

  let { value = $bindable(''), textarea = $bindable(null), diagnostics = [], oninput, onkeydown } = $props();
  const noteId = `bd-source-error-${++nextNoteId}`;
  let backdrop;
  function syncScroll() {
    if (backdrop && textarea) { backdrop.scrollTop = textarea.scrollTop; backdrop.scrollLeft = textarea.scrollLeft; }
  }
  const pieces = $derived.by(() => {
    let offset = 0;
    return diagnosticPieces(value, diagnostics).map((piece) => {
      const start = offset;
      offset += piece.text.length;
      return {...piece, start, end:offset};
    });
  });
  const messages = $derived([...new Set(diagnostics.map((range) => range.message).filter(Boolean))]);
</script>

<div class="bd-source-editor">
  <div class="bd-source-field">
    <div class="bd-source-highlights" bind:this={backdrop} aria-hidden="true">{#each pieces as piece}{#if piece.marked}<mark class:insertion={piece.point} data-source-start={piece.start} data-source-end={piece.end}>{piece.point ? '\u200b' : piece.text}</mark>{:else}{piece.text}{/if}{/each}{'\n'}</div>
    <textarea
      class="bd-source"
      bind:this={textarea}
      bind:value
      {oninput}
      {onkeydown}
      onscroll={syncScroll}
      rows="1"
      spellcheck="false"
      autocapitalize="off"
      aria-label="Markdown source of this block"
      aria-invalid={diagnostics.length > 0 ? 'true' : undefined}
      aria-describedby={messages.length ? noteId : undefined}
    ></textarea>
  </div>
  {#if messages.length}<div class="bd-source-diagnostics" id={noteId}>{#each messages as message}<div>{message}</div>{/each}</div>{/if}
</div>

<style>
  .bd-source-editor { margin:0.4em 0; }
  .bd-source-field {
    position:relative;
    font-family:var(--bd-source-font,ui-monospace,SFMono-Regular,Menlo,Consolas,monospace);
    font-size:var(--bd-source-size,0.92em);
    line-height:1.55;
    background:var(--bd-source-bg,transparent);
    border-radius:var(--bd-radius,4px);
  }
  .bd-source, .bd-source-highlights {
    box-sizing:border-box;
    width:100%;
    margin:0;
    padding:var(--bd-source-padding,0.35em 0.5em);
    font:inherit;
    letter-spacing:inherit;
    text-align:inherit;
    white-space:pre-wrap;
    overflow-wrap:break-word;
    word-break:normal;
    tab-size:4;
    border:1px solid transparent;
  }
  .bd-source-highlights { position:absolute; inset:0; color:transparent; pointer-events:none; overflow:hidden; }
  mark {
    padding:0;
    color:transparent;
    background:color-mix(in srgb,var(--bd-error,Crimson) 20%,transparent);
    text-decoration:underline wavy var(--bd-error,Crimson);
    text-underline-offset:3px;
  }
  mark.insertion { border-left:2px solid var(--bd-error,Crimson); margin:0 -1px; text-decoration:none; }
  .bd-source { position:relative; display:block; color:inherit; background:transparent; border-color:var(--bd-source-border,currentColor); border-radius:var(--bd-radius,4px); resize:none; overflow:hidden; }
  .bd-source:focus { outline:none; border-color:var(--bd-accent,Highlight); }
  .bd-source-diagnostics { padding:0.4em 0.1em; color:var(--bd-error,Crimson); font-size:0.8em; line-height:1.5; }
  @media (forced-colors:active) { mark { background:transparent; text-decoration-color:CanvasText; forced-color-adjust:none; } mark.insertion { border-left-color:CanvasText; } }
</style>
