/** Findings name either the document ID or its on-disk path before ': '. */
export function documentProblems(problems, id, path) {
  if (!id) return problems;
  return problems.filter((problem) => {
    const location = problem.split(': ', 1)[0];
    return location === id || (path && (location === path || location.endsWith(`/${path}`)));
  });
}
