//! The status graph: which move a type allows, and how a refusal is worded.
//!
//! A type's `[types.<name>.transitions]` map (ADR-0050 decision 1) is read
//! *only* through here, so the four places that care — `set-status`'s guard,
//! `close`'s guard, `verify`'s history-pair check and the node's status menu —
//! cannot drift apart on declared order, duplicate targets, or what an absent
//! map means. Two rules carry the whole module:
//!
//! - a type with **no** map keeps the any-to-any rule byte-identically
//!   ([`allowed_targets`] returns `None`), so every existing config is
//!   unaffected;
//! - within a map, a status that is not a key is a **dead end**
//!   (`Some(vec![])`), not an omission that means "anything goes". Half a map
//!   would otherwise be the most dangerous shape: an author who declares the
//!   two edges they care about would silently leave every other status wide
//!   open.
//!
//! Terminal statuses are deliberately *not* filtered out here. An edge into one
//! is real — it is what [`can_close`] reads — and it is `set-status`'s own
//! terminal guard, not this map, that keeps `close` the only door. A caller
//! offering the user a menu (the node's `DocView`) filters them itself.

use crate::project_config::DocType;

/// The statuses `from` may move to, or `None` when the type declares no
/// transitions at all (any-to-any). `Some(vec![])` is a dead end.
///
/// The targets come back in declared order with duplicates removed: that order
/// is what every refusal message and the node's status menu show, so it is the
/// author's order, never a sorted one.
pub fn allowed_targets(t: &DocType, from: &str) -> Option<Vec<String>> {
    if t.transitions.is_empty() {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    for target in t.transitions.get(from).map(Vec::as_slice).unwrap_or(&[]) {
        if !out.iter().any(|s| s == target) {
            out.push(target.clone());
        }
    }
    Some(out)
}

/// Whether the type allows `from -> to`. A type with no map allows everything.
///
/// A move to the status a document already holds is **not** an edge and is
/// never consulted: `set-status <id> <its current status>` has always been a
/// legal no-op that appends a history entry, and a uniform edge check would turn
/// it into a refusal for want of a self-edge nobody would think to declare. The
/// callers skip the equal case; this function is only ever asked about a real
/// move.
pub fn allows(t: &DocType, from: &str, to: &str) -> bool {
    allowed_targets(t, from).is_none_or(|targets| targets.iter().any(|s| s == to))
}

/// Whether `close` may run from `from`: the type declares no transitions (the
/// pre-ADR-0050 rule, where only `terminal_statuses` gates closing), or `from`
/// has a declared edge into one of the type's terminal statuses.
pub fn can_close(t: &DocType, from: &str) -> bool {
    match allowed_targets(t, from) {
        None => true,
        Some(targets) => targets.iter().any(|s| t.terminal_statuses.contains(s)),
    }
}

/// The statuses of `t` that `close` may be run from, in declared order — the
/// "…only X can close" half of [`close`](crate::commands::close)'s refusal.
pub fn closing_statuses(t: &DocType) -> Vec<String> {
    t.statuses
        .iter()
        .filter(|s| !t.terminal_statuses.contains(s) && can_close(t, s))
        .cloned()
        .collect()
}

/// The parenthetical every refusal and every verify finding ends with: the
/// declared targets of `from`, or a plain statement that there are none.
///
/// One wording in one place, because a user who hits the guard twice — once from
/// the CLI, once from the node — must not have to work out that two differently
/// phrased sentences are the same rule.
pub fn targets_phrase(t: &DocType, from: &str) -> String {
    match allowed_targets(t, from) {
        Some(targets) if !targets.is_empty() => {
            format!("from {from}: {}", targets.join(", "))
        }
        _ => format!("no transitions out of {from}"),
    }
}

/// The same phrase for a caller that is refusing a `set-status`: the targets a
/// user can actually reach with that command, so the advice is followable.
///
/// [`targets_phrase`] lists every declared edge, terminal ones included,
/// because `close` and verify's history findings need them. `set-status`
/// refuses a terminal target outright, so naming one there sends the user
/// straight into a second refusal; the edge into it is `close`'s, and this
/// says so instead.
pub fn set_status_targets_phrase(t: &DocType, from: &str) -> String {
    match allowed_targets(t, from) {
        Some(targets) => {
            let (terminal, open): (Vec<String>, Vec<String>) = targets
                .into_iter()
                .partition(|s| t.terminal_statuses.iter().any(|term| term == s));
            let close_hint = if terminal.is_empty() {
                String::new()
            } else {
                format!("; `opys close` reaches {}", terminal.join(", "))
            };
            if open.is_empty() {
                format!("no transitions out of {from}{close_hint}")
            } else {
                format!("from {from}: {}{close_hint}", open.join(", "))
            }
        }
        None => format!("no transitions out of {from}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_type(toml: &str) -> DocType {
        let cfg: crate::project_config::ProjectConfig =
            toml::from_str(&format!("[types.t]\nprefix = \"T\"\n{toml}")).expect("parses");
        cfg.types.into_values().next().expect("one type")
    }

    /// No map at all is the pre-ADR-0050 world: every move allowed, and the
    /// caller can tell "anything" from "nothing" by the `None`.
    #[test]
    fn a_type_without_transitions_allows_everything() {
        let t = doc_type("statuses = [\"a\", \"b\"]\ndefault_status = \"a\"\n");
        assert_eq!(allowed_targets(&t, "a"), None);
        assert!(allows(&t, "a", "b"));
        assert!(can_close(&t, "a"));
    }

    /// Declared order survives, duplicates do not, and a status the map does not
    /// mention is a dead end rather than a wildcard.
    #[test]
    fn targets_keep_declared_order_and_drop_duplicates() {
        let t = doc_type(
            "statuses = [\"a\", \"b\", \"c\"]\ndefault_status = \"a\"\n\
             [types.t.transitions]\na = [\"c\", \"b\", \"c\"]\n",
        );
        assert_eq!(
            allowed_targets(&t, "a"),
            Some(vec!["c".to_string(), "b".to_string()])
        );
        assert_eq!(allowed_targets(&t, "b"), Some(Vec::new()));
        assert!(!allows(&t, "b", "a"));
        assert_eq!(targets_phrase(&t, "a"), "from a: c, b");
        assert_eq!(targets_phrase(&t, "b"), "no transitions out of b");
    }

    /// A `set-status` refusal must not advise a move `set-status` refuses: the
    /// terminal target moves into the `close` hint.
    #[test]
    fn a_set_status_refusal_does_not_advise_a_terminal_target() {
        let t = doc_type(
            "statuses = [\"todo\", \"review\", \"done\"]\ndefault_status = \"todo\"\n\
             terminal_statuses = [\"done\"]\n\
             [types.t.transitions]\ntodo = [\"review\"]\nreview = [\"done\", \"todo\"]\n",
        );
        assert_eq!(targets_phrase(&t, "review"), "from review: done, todo");
        assert_eq!(
            set_status_targets_phrase(&t, "review"),
            "from review: todo; `opys close` reaches done"
        );
        assert_eq!(set_status_targets_phrase(&t, "todo"), "from todo: review");
    }

    /// `close` reads the edges into terminal statuses — the one thing
    /// [`allowed_targets`] must not filter away.
    #[test]
    fn closing_needs_an_edge_into_a_terminal_status() {
        let t = doc_type(
            "statuses = [\"todo\", \"review\", \"done\"]\ndefault_status = \"todo\"\n\
             terminal_statuses = [\"done\"]\n\
             [types.t.transitions]\ntodo = [\"review\"]\nreview = [\"done\", \"todo\"]\n",
        );
        assert!(!can_close(&t, "todo"));
        assert!(can_close(&t, "review"));
        assert_eq!(closing_statuses(&t), vec!["review".to_string()]);
    }
}
