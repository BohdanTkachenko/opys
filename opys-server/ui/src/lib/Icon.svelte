<script>
  // The icon set: hand-kept 24×24 stroke paths, rendered inline in
  // `currentColor` so every icon inherits the text colour around it.
  //
  // Inline SVG and not a font or a sprite request, because the page makes zero
  // external requests — and not a dependency, because an icon library would be
  // the first one the UI has and these few shapes do not justify it.
  // Stroke width 1.75 reads crisply at the 14–16px these are drawn at.

  const PATHS = {
    search: 'M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14Zm9 16-4.05-4.05',
    board: 'M5 5h4v14H5zM10.5 5h4v9h-4zM16 5h4v6h-4z',
    terminal: 'm5 7 4 4-4 4M11 16h8',
    doc: 'M7 3h7l4 4v14H7zM14 3v4h4',
    // Two columns side by side. The earlier pair of opposed arrows turned to
    // a smudge at the 13px the sidebar draws it.
    compare: 'M4 5h6.5v14H4zM13.5 5H20v14h-6.5z',
    menu: 'M4 7h16M4 12h16M4 17h16',
    flag: 'M5 21V4h11l-1.5 3.5L16 11H5',
    tag: 'm12 3 8 8-9 9-8-8V4h8Zm-4 4h.01',
    clock: 'M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16Zm0 4v4l3 2',
    plus: 'M12 5v14M5 12h14',
    x: 'M6 6l12 12M18 6 6 18',
    chevron: 'm6 9 6 6 6-6',
    alert: 'M12 3 2.5 20h19L12 3Zm0 7v4m0 3h.01',
    check: 'm4.5 12.5 5 5 10-11',
    folder: 'M3 6a1.5 1.5 0 0 1 1.5-1.5h5L12 7h7.5A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5z',
    zap: 'M13 2 4 14h6l-1 8 9-12h-6z',
    lock: 'M7 11V8a5 5 0 0 1 10 0v3M6 11h12v9H6z',
    home: 'm3 11 9-8 9 8M6 10v10h12V10',
  };

  let { name, size = 15 } = $props();
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.75"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <path d={PATHS[name] ?? ''} />
</svg>

<style>
  .icon {
    flex: none;
    vertical-align: -0.18em;
  }
</style>
