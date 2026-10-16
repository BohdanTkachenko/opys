// pi harness extension: inject the canonical opys rule into the agent's system
// prompt. The rule is self-gating (it tells the agent to do nothing unless the
// project has a docs/opys/ inventory), and is read from the single source of
// truth rather than duplicated here.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const rule = readFileSync(
  join(here, "..", "skills", "opys", "agent-rule.md"),
  "utf8",
).trim();

export default function opysExtension(pi) {
  pi.on("before_agent_start", async (event) => ({
    systemPrompt: `${event.systemPrompt}\n\n${rule}`,
  }));
}
