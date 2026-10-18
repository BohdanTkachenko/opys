// All offsets here are JavaScript string offsets (UTF-16), like textarea
// selections and block segmentation. Hosts translate their source coordinates.

/** The exact text being typed, plus the open field's range in that snapshot. */
export function draftSnapshot(source, edit, draft) {
  if (!edit.insert) return {
    source:source.slice(0,edit.start) + draft + source.slice(edit.end),
    start:edit.start, end:edit.start + draft.length,
  };
  if (source.trim() === '') return {source:draft + '\n', start:0, end:draft.length};
  if (edit.start === 0) return {source:draft + '\n\n' + source.replace(/^\n+/,''), start:0, end:draft.length};
  const start = edit.start + 2;
  return {source:source.slice(0,edit.start) + '\n\n' + draft + source.slice(edit.start), start, end:start + draft.length};
}

/** Keep untouched diagnostics aligned with a draft; never guess inside edits. */
export function mapDiagnosticRanges(original, draft, diagnostics) {
  if (original === draft) return diagnostics;
  let prefix = 0;
  while (prefix < original.length && prefix < draft.length && original[prefix] === draft[prefix]) prefix++;
  let suffix = 0;
  while (suffix < original.length - prefix && suffix < draft.length - prefix
    && original[original.length - suffix - 1] === draft[draft.length - suffix - 1]) suffix++;
  const oldEnd = original.length - suffix;
  const shift = draft.length - original.length;
  return diagnostics.flatMap((range) => {
    if (range.start === range.end) {
      if (range.start < prefix) return [range];
      if (range.start > oldEnd) return [{...range, start:range.start + shift, end:range.end + shift}];
      return [];
    }
    if (range.end <= prefix) return [range];
    if (range.start >= oldEnd) return [{...range, start:range.start + shift, end:range.end + shift}];
    return [];
  });
}

/** Source pieces for a mirror behind a textarea, including zero-width marks. */
export function diagnosticPieces(text, diagnostics) {
  const ranges = diagnostics.filter((range) => Number.isInteger(range.start) && Number.isInteger(range.end)
    && range.start >= 0 && range.start <= range.end && range.end <= text.length);
  const points = new Set(ranges.filter((range) => range.start === range.end).map((range) => range.start));
  const boundaries = [...new Set([0, text.length, ...ranges.flatMap((range) => [range.start, range.end])])].sort((a,b) => a-b);
  const pieces = [];
  for (let i = 0; i < boundaries.length; i++) {
    const start = boundaries[i], end = boundaries[i + 1];
    if (points.has(start)) pieces.push({text:'', marked:true, point:true});
    if (end !== undefined && end > start) pieces.push({
      text:text.slice(start,end), marked:ranges.some((range) => range.start < end && start < range.end), point:false,
    });
  }
  return pieces;
}

export function diagnosticsInBlock(diagnostics, start, end) {
  return diagnostics.flatMap((range) => {
    const point = range.start === range.end;
    if (point ? range.start < start || range.start > end : range.start >= end || range.end <= start) return [];
    return [{...range, start:Math.max(start,range.start)-start, end:Math.min(end,range.end)-start}];
  });
}
