//! # mdprism — markdown ⇄ data, via a template
//!
//! One compact schema (a textual DSL) defines a bidirectional mapping between a
//! markdown document and a typed data object. From it you can **validate**,
//! **extract** (parse → data), **render** (data → markdown), **scaffold**,
//! **query**, and **edit in-place**.
//!
//! See `docs/structure-dsl-spec.md` for the language and `docs/mdprism-reference.md`
//! for a worked example.
//!
//! ## Status
//!
//! v0 (this crate) implements the **schema parser** ([`parse_schema`]) and the
//! [`Schema`] data model. Validation, extraction, rendering, scaffolding,
//! querying, and editing are the next phases (they will pull in `comrak` for the
//! document AST and `jaq` for queries).

mod error;
mod parse;
mod schema;

pub use error::{Problem, SchemaError};
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
}
