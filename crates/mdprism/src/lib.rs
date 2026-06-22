//! # mdprism — markdown ⇄ data, via a template
//!
//! One compact schema (a textual DSL) defines a bidirectional mapping between a
//! markdown document and a typed data object. From it you can **validate**,
//! **extract** (parse → data), **render** (data → markdown), **scaffold**,
//! **query** (jq via jaq), and **edit in-place**.
//!
//! See `docs/structure-dsl-spec.md` for the language and `docs/mdprism-reference.md`
//! for a worked example.
//!
//! ## Status
//!
//! Implemented: schema parser ([`parse_schema`]), [`Schema`] data model,
//! **`scaffold`** (starter document), **`validate`** / **`extract`** (body
//! conformance), **`render`** (data → markdown), and **`query`** (jq via jaq).
//! Next phase: in-place `edit` using comrak sourcepos.

mod edit;
mod error;
mod parse;
mod query;
mod render;
mod schema;
mod validate;

pub use error::{EditError, Problem, QueryError, RenderError, SchemaError, ValidationErrors};
pub use parse::parse_schema;
pub use schema::{Card, FieldSchema, FieldType, Head, ListStyle, Match, Node, Schema, SchemaOpts};

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Schema {
        parse_schema(src).expect("schema parses")
    }

    #[test]
    fn defaults_are_strict() {
        let s = parse("> @x");
        assert!(s.opts.ordered && s.opts.strict && !s.opts.frontmatter_open);
    }

    #[test]
    fn directives_override_defaults() {
        let s = parse("%ordered = false\n%strict = false\n%frontmatter = open\n> @x");
        assert!(!s.opts.ordered && !s.opts.strict && s.opts.frontmatter_open);
    }

    #[test]
    fn frontmatter_types_and_alias() {
        let s = parse(
            "---\n\
             title: string\n\
             status: enum(planned, done)\n\
             tags: [string]+\n\
             owner?: string\n\
             spec_url? @spec: /^https/\n\
             ---\n",
        );
        let f = &s.frontmatter;
        assert_eq!(f.len(), 5);
        assert_eq!(f[0].key, "title");
        assert_eq!(f[0].ty, FieldType::Str);
        assert_eq!(
            f[1].ty,
            FieldType::Enum(vec!["planned".into(), "done".into()])
        );
        assert_eq!(f[2].ty, FieldType::List(Box::new(FieldType::Str)));
        assert!(f[3].optional && f[3].alias == "owner");
        // alias override + regex type
        assert_eq!(f[4].key, "spec_url");
        assert_eq!(f[4].alias, "spec");
        assert!(f[4].optional);
        assert!(matches!(f[4].ty, FieldType::Regex(_)));
    }

    #[test]
    fn body_head_card_name_label() {
        let s = parse(
            "## @manual Manual verification\n\
            \x20 ### @setup Setup\n\
            \x20   - +@items\n\
            \x20 ### @procedure Procedure\n\
            \x20   1. +@steps\n\
            \x20     - ?@note\n",
        );
        // top level: one heading "manual" with two child headings
        assert_eq!(s.body.len(), 1);
        let Node::Heading {
            level,
            title,
            head,
            children,
        } = &s.body[0]
        else {
            panic!("expected heading");
        };
        assert_eq!(*level, 2);
        assert_eq!(head.name.as_deref(), Some("manual"));
        assert_eq!(*title, Match::Literal("Manual verification".into()));
        assert_eq!(children.len(), 2);

        // setup -> a "+@items" bullet list
        let Node::Heading {
            children: setup, ..
        } = &children[0]
        else {
            panic!();
        };
        assert_eq!(setup.len(), 1);
        let Node::List { style, head, .. } = &setup[0] else {
            panic!("expected list");
        };
        assert_eq!(*style, ListStyle::Bullet);
        assert_eq!(head.name.as_deref(), Some("items"));
        assert_eq!(head.card, Card::Plus);

        // procedure -> ordered list -> optional nested bullet
        let Node::Heading { children: proc, .. } = &children[1] else {
            panic!();
        };
        let Node::List {
            style,
            children: steps_kids,
            ..
        } = &proc[0]
        else {
            panic!();
        };
        assert_eq!(*style, ListStyle::Ordered);
        assert_eq!(steps_kids.len(), 1);
        let Node::List { head, .. } = &steps_kids[0] else {
            panic!();
        };
        assert_eq!(head.card, Card::Optional);
        assert_eq!(head.name.as_deref(), Some("note"));
    }

    #[test]
    fn labels_regex_literal_and_bare_with_card() {
        // regex label
        let s = parse("# @title /.+/");
        let Node::Heading { title, .. } = &s.body[0] else {
            panic!();
        };
        assert_eq!(*title, Match::Regex(".+".into()));

        // bare literal label with a leading card, no name
        let s = parse("## + Docs:");
        let Node::Heading { title, head, .. } = &s.body[0] else {
            panic!();
        };
        assert_eq!(head.card, Card::Plus);
        assert_eq!(head.name, None);
        assert_eq!(*title, Match::Literal("Docs:".into()));

        // range card
        let s = parse("- {1,5}@points");
        let Node::List { head, .. } = &s.body[0] else {
            panic!();
        };
        assert_eq!(head.card, Card::Range(1, Some(5)));
        assert_eq!(head.name.as_deref(), Some("points"));
    }

    #[test]
    fn description_is_fenced_and_label_may_contain_dashes() {
        let s = parse("## @t Trade-offs: speed -- vs -- safety   <? the desc ?>");
        let Node::Heading { title, head, .. } = &s.body[0] else {
            panic!();
        };
        assert_eq!(
            *title,
            Match::Literal("Trade-offs: speed -- vs -- safety".into())
        );
        assert_eq!(head.desc.as_deref(), Some("the desc"));
    }

    #[test]
    fn escaping_leading_special() {
        let s = parse("- \\+literal-plus");
        let Node::List { item, head, .. } = &s.body[0] else {
            panic!();
        };
        assert_eq!(head.card, Card::Required); // the '+' was escaped, not a card
        assert_eq!(*item, Some(Match::Literal("+literal-plus".into())));
    }

    #[test]
    fn unterminated_frontmatter_errors() {
        let err = parse_schema("---\ntitle: string\n").unwrap_err();
        assert!(err.message.contains("unterminated frontmatter"));
    }

    const SCHEMA: &str = "## @manual Manual verification\n\
        \x20 ### @setup Setup\n\
        \x20   - +@items\n\
        \x20 ### @procedure Procedure\n\
        \x20   1. +@steps\n\
         ## @plan Test plan\n\
        \x20 - [ ] +@cases\n";

    #[test]
    fn validate_accepts_a_conforming_document() {
        let s = parse(SCHEMA);
        let doc = "## Manual verification\n\
            ### Setup\n\
            - external monitor\n\n\
            ### Procedure\n\
            1. open a tab\n\
            2. run it\n\n\
            ## Test plan\n\
            - [x] one\n\
            - [ ] two\n";
        let problems = s.validate(doc);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn validate_flags_missing_section_and_empty_list() {
        let s = parse(SCHEMA);
        // No "Test plan" section; Setup present but with no items.
        let doc = "## Manual verification\n\
            ### Setup\n\n\
            ### Procedure\n\
            1. open\n";
        let problems = s.validate(doc);
        let joined: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
        let all = joined.join("\n");
        assert!(all.contains("plan"), "missing Test plan: {all}");
        assert!(
            all.contains("setup") && all.contains("at least one item"),
            "empty Setup list: {all}"
        );
    }

    #[test]
    fn validate_respects_strict_ordering() {
        // Schema wants Setup then Procedure; doc has them reversed.
        let s = parse("## @m M\n  ### @setup Setup\n    > @x\n  ### @proc Procedure\n    > @y\n");
        let reversed = "## M\n### Procedure\nbody\n\n### Setup\nbody\n";
        let problems = s.validate(reversed);
        assert!(!problems.is_empty(), "strict order should flag reversal");
    }

    #[test]
    fn extract_builds_the_capture_object() {
        let s = parse(SCHEMA);
        let doc = "## Manual verification\n\
            ### Setup\n\
            - external monitor\n\n\
            ### Procedure\n\
            1. open a tab\n\
            2. run it\n\n\
            ## Test plan\n\
            - [x] one\n\
            - [ ] two\n";
        let v = s.extract(doc).expect("conforms");
        assert_eq!(v["manual"]["setup"]["items"][0], "external monitor");
        assert_eq!(v["manual"]["procedure"]["steps"][1], "run it");
        assert_eq!(v["plan"]["cases"], serde_json::json!(["one", "two"]));
    }

    #[test]
    fn extract_errors_on_nonconforming() {
        let s = parse(SCHEMA);
        let bad = "## Manual verification\n### Setup\n\n### Procedure\n1. x\n";
        assert!(s.extract(bad).is_err());
    }

    #[test]
    fn scaffold_emits_required_only() {
        let s = parse(
            "---\n\
             title: string\n\
             status: enum(planned, done)\n\
             tags: [string]+\n\
             owner?: string\n\
             ---\n\
             ## @manual Manual verification\n\
            \x20 ### @setup Setup\n\
            \x20   - +@items\n\
             ## ?@refs References\n\
            \x20 - *@links\n",
        );
        let out = s.scaffold();
        assert!(out.contains("title: \n"), "{out}");
        assert!(out.contains("status: planned\n"), "{out}"); // first enum value
        assert!(out.contains("tags: []\n"), "{out}");
        assert!(!out.contains("owner"), "optional key omitted: {out}");
        assert!(out.contains("## Manual verification\n"), "{out}");
        assert!(out.contains("### Setup\n"), "{out}");
        assert!(out.contains("- \n") || out.contains("- "), "{out}");
        assert!(
            !out.contains("References"),
            "optional heading omitted: {out}"
        );
    }

    #[test]
    fn render_produces_conforming_document() {
        let s = parse(SCHEMA);
        let data = serde_json::json!({
            "manual": {
                "setup": { "items": ["external monitor", "test device"] },
                "procedure": { "steps": ["open a tab", "run it"] }
            },
            "plan": { "cases": ["smoke", "edge case"] }
        });
        let md = s.render(&data).expect("renders");
        // Re-validate the rendered output
        let problems = s.validate(&md);
        assert!(problems.is_empty(), "rendered doc fails validate: {problems:?}\n---\n{md}");
    }

    #[test]
    fn render_errors_on_missing_required_field() {
        let s = parse(SCHEMA);
        // Missing "plan" (required section)
        let data = serde_json::json!({
            "manual": {
                "setup": { "items": ["a"] },
                "procedure": { "steps": ["b"] }
            }
        });
        let err = s.render(&data).unwrap_err();
        assert!(matches!(err, RenderError::MissingField(_)));
    }

    #[test]
    fn query_extracts_and_filters() {
        let s = parse(SCHEMA);
        let doc = "## Manual verification\n\
            ### Setup\n\
            - external monitor\n\n\
            ### Procedure\n\
            1. open a tab\n\
            2. run it\n\n\
            ## Test plan\n\
            - [x] one\n\
            - [ ] two\n";
        let results = s.query(doc, ".plan.cases[]").expect("query succeeds");
        assert_eq!(results, vec!["one", "two"]);
    }

    #[test]
    fn edit_replaces_list_item_text() {
        let s = parse(SCHEMA);
        let doc = "## Manual verification\n\
            ### Setup\n\
            - external monitor\n\n\
            ### Procedure\n\
            1. open a tab\n\
            2. run it\n\n\
            ## Test plan\n\
            - [x] one\n\
            - [ ] two\n";
        // Replace the first setup item.
        let edited = s.edit(doc, "manual.setup.items.0", "projector").expect("edit succeeds");
        assert!(edited.contains("- projector\n"), "item replaced: {edited}");
        assert!(!edited.contains("external monitor"), "old text gone: {edited}");
        // Rest of the document is untouched.
        assert!(edited.contains("open a tab"), "rest preserved: {edited}");
        // Re-validates.
        let problems = s.validate(&edited);
        assert!(problems.is_empty(), "edited doc conforms: {problems:?}");
    }

    #[test]
    fn edit_replaces_checklist_item_preserving_checkbox() {
        let s = parse(SCHEMA);
        let doc = "## Manual verification\n\
            ### Setup\n\
            - a\n\n\
            ### Procedure\n\
            1. step\n\n\
            ## Test plan\n\
            - [x] old case\n\
            - [ ] second\n";
        let edited = s.edit(doc, "plan.cases.0", "new case").expect("edit succeeds");
        // The `[x]` prefix must be preserved.
        assert!(edited.contains("- [x] new case\n"), "checkbox preserved: {edited}");
        assert!(!edited.contains("old case"), "old text gone: {edited}");
    }

    #[test]
    fn edit_returns_target_not_found_for_bad_path() {
        let s = parse(SCHEMA);
        let doc = "## Manual verification\n\
            ### Setup\n\
            - a\n\n\
            ### Procedure\n\
            1. b\n\n\
            ## Test plan\n\
            - [ ] c\n";
        let err = s.edit(doc, "nonexistent.path", "x").unwrap_err();
        assert!(matches!(err, EditError::TargetNotFound));
    }
}
