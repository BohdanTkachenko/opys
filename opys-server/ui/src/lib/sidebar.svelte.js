// Whether the wide-screen sidebar is folded into a rail.
//
// Shared state rather than the sidebar's own, because the shell owns the grid
// the sidebar sits in: folding it has to narrow the column, not just empty it.
// The narrow-screen bar (under 46rem) has its own open/closed toggle and
// ignores this entirely — there the sidebar is already a one-line bar.
//
// The choice persists. With none stored, a screen that leaves the main column
// less than a board's worth of room (under 64rem) starts folded: 17rem of
// project list beside three squeezed columns is the complaint this answers.

const KEY = 'opys:sidebar:collapsed';

function initial() {
  try {
    const stored = localStorage.getItem(KEY);
    if (stored !== null) return stored === '1';
  } catch {
    // Private browsing: fall through to the width default.
  }
  return typeof window !== 'undefined' && window.matchMedia?.('(max-width: 64rem)').matches === true;
}

let collapsed = $state(initial());

export const sidebar = {
  get collapsed() {
    return collapsed;
  },
  toggle() {
    collapsed = !collapsed;
    try {
      localStorage.setItem(KEY, collapsed ? '1' : '0');
    } catch {
      // The toggle still works for this visit.
    }
  },
};
