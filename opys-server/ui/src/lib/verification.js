/** Findings name either the document ID or its on-disk path before ': '. */
export function documentProblems(problems, id, path) {
  if (!id) return problems;
  return problems.filter((problem) => {
    const location = problem.split(': ', 1)[0];
    return location === id || (path && (location === path || location.endsWith(`/${path}`)));
  });
}

/** Structured findings are additive; older nodes still supply message strings. */
export function verificationFindings(report, id, path) {
  const findings = report?.findings ?? (report?.problems ?? []).map((message) => ({ message }));
  if (!id) return findings;
  return findings.filter((finding) => finding.doc_id
    ? finding.doc_id === id
    : (path && finding.path && (finding.path === path || finding.path.endsWith(`/${path}`)))
      || documentProblems([finding.message], id, path).length > 0);
}

/** Split source using Unicode scalar columns; Svelte escapes all three pieces. */
export function sourceLines(finding) {
  if (!finding.range || !finding.excerpt) return [];
  const { start, end } = finding.range;
  const point = start.line === end.line && start.column === end.column;
  return finding.excerpt.text.split('\n').map((text, index) => {
    text = text.replace(/\r$/, '');
    const number = finding.excerpt.first_line + index;
    const chars = Array.from(text);
    const from = number === start.line ? start.column - 1 : 0;
    const to = number === end.line ? end.column - 1 : chars.length;
    const selected = number >= start.line && number <= end.line
      && (point || number < end.line || end.column > 1);
    return {
      number, selected, point: point && selected,
      before: selected ? chars.slice(0, from).join('') : text,
      marked: selected ? chars.slice(from, to).join('') : '',
      after: selected ? chars.slice(to).join('') : '',
    };
  });
}

function positionOffset(text, position) {
  let start = 0;
  for (let line = 1; line < position.line; line++) {
    const next = text.indexOf('\n', start);
    if (next < 0) return text.length;
    start = next + 1;
  }
  const end = text.indexOf('\n', start);
  const content = text.slice(start, end < 0 ? text.length : end);
  return start + Array.from(content).slice(0, position.column - 1).join('').length;
}

/** Convert verified scalar coordinates to the editor's UTF-16 source offsets. */
export function editorDiagnostics(body, version, findings) {
  if (!version) return [];
  return findings.filter((finding) => finding?.body_range && finding.body_version === version).map((finding) => ({
    start:positionOffset(body, finding.body_range.start),
    end:positionOffset(body, finding.body_range.end),
    message:finding.message,
  }));
}
