//! The accessibility tree, through the reader in the box.
//!
//! Shared by both display servers rather than written twice: AT-SPI sits at the
//! toolkit, so the same tree comes back whether X11 or Wayland drew the window,
//! and the command that reads it is the same command.

use crate::error::{Error, Result};
use crate::machine::ScreenHost;
use crate::{Node, NodeQuery, ScreenId};
use std::sync::Arc;

/// The reader, as the image installs it.
const READER: &str = "computer-a11y";

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

/// What a query narrows the search by, without the words it searches for:
/// `set` takes a second positional, so the flags cannot be bundled with them.
fn flags_of(query: &NodeQuery) -> Vec<String> {
    let mut flags = Vec::new();

    if let Some(role) = &query.role {
        flags.extend(argv(&["--role", role]));
    }
    if let Some(app) = &query.app {
        flags.extend(argv(&["--app", app]));
    }
    if query.exact {
        flags.push("--exact".to_string());
    }

    flags
}

async fn read(host: &Arc<dyn ScreenHost>, screen: ScreenId, args: Vec<String>) -> Result<String> {
    let result = host.run(&args, screen).await?;

    if result.code != 0 {
        let said = result.stderr_utf8().trim().to_string();

        // 127 is the shell's "no such command", which here means an image
        // built without the feature rather than a call that was wrong.
        if result.code == 127 {
            return Err(Error::denied(
                "this box has no accessibility tree: it was built without \
                 Feature::Accessibility, and a running box cannot be given one \
                 — an application joins the tree only if the bus was there \
                 before it drew",
            ));
        }

        return Err(Error::denied(said));
    }

    Ok(result.stdout_utf8())
}

fn nodes_from(json: &str) -> Result<Vec<Node>> {
    #[derive(serde::Deserialize)]
    struct Answer {
        nodes: Vec<Node>,
    }

    serde_json::from_str::<Answer>(json)
        .map(|answer| answer.nodes)
        .map_err(|error| Error::denied(format!("the tree would not parse: {error}")))
}

fn node_from(json: &str) -> Result<Node> {
    #[derive(serde::Deserialize)]
    struct Answer {
        node: Node,
    }

    serde_json::from_str::<Answer>(json)
        .map(|answer| answer.node)
        .map_err(|error| Error::denied(format!("the answer would not parse: {error}")))
}

pub async fn tree(
    host: &Arc<dyn ScreenHost>,
    screen: ScreenId,
    app: Option<&str>,
    depth: Option<u32>,
) -> Result<Vec<Node>> {
    let mut args = argv(&[READER, "tree"]);
    if let Some(app) = app {
        args.extend(argv(&["--app", app]));
    }
    if let Some(depth) = depth {
        args.extend(argv(&["--depth", &depth.to_string()]));
    }

    nodes_from(&read(host, screen, args).await?)
}

pub async fn find(
    host: &Arc<dyn ScreenHost>,
    screen: ScreenId,
    query: &NodeQuery,
    limit: Option<usize>,
) -> Result<Vec<Node>> {
    let mut args = argv(&[READER, "find", &query.query]);
    args.extend(flags_of(query));
    if let Some(limit) = limit {
        args.extend(argv(&["--limit", &limit.to_string()]));
    }

    nodes_from(&read(host, screen, args).await?)
}

pub async fn focus(
    host: &Arc<dyn ScreenHost>,
    screen: ScreenId,
    query: &NodeQuery,
) -> Result<Node> {
    let mut args = argv(&[READER, "focus", &query.query]);
    args.extend(flags_of(query));

    node_from(&read(host, screen, args).await?)
}

pub async fn invoke(
    host: &Arc<dyn ScreenHost>,
    screen: ScreenId,
    query: &NodeQuery,
    action: Option<&str>,
) -> Result<Node> {
    let mut args = argv(&[READER, "invoke", &query.query]);
    args.extend(flags_of(query));
    if let Some(action) = action {
        args.extend(argv(&["--action", action]));
    }

    node_from(&read(host, screen, args).await?)
}

pub async fn set(
    host: &Arc<dyn ScreenHost>,
    screen: ScreenId,
    query: &NodeQuery,
    value: &str,
) -> Result<Node> {
    let mut args = argv(&[READER, "set", &query.query, value]);
    args.extend(flags_of(query));

    node_from(&read(host, screen, args).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_query_narrows_by_only_what_it_was_given() {
        let bare = NodeQuery {
            query: "Place order".to_string(),
            ..NodeQuery::default()
        };
        assert!(flags_of(&bare).is_empty(), "a bare query narrows nothing");

        let narrowed = NodeQuery {
            query: "OK".to_string(),
            role: Some("push button".to_string()),
            app: Some("zenity".to_string()),
            exact: true,
        };
        assert_eq!(
            flags_of(&narrowed),
            argv(&["--role", "push button", "--app", "zenity", "--exact"]),
            "and the words themselves are a positional, not a flag"
        );
    }

    #[test]
    fn test_a_tree_with_no_bounds_still_parses() {
        // The reader leaves out what a widget does not publish, and a menu
        // item that has never been drawn has no rectangle.
        let nodes = nodes_from(
            r#"{"nodes":[{"id":"0.1","app":"zenity","role":"menu item",
                          "name":"Open","actions":["click"],"states":["enabled"]}]}"#,
        )
        .expect("parses");

        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].at.is_none(), "no rectangle rather than 0,0");
        assert_eq!(nodes[0].width, 0);
    }

    #[test]
    fn test_a_node_says_which_words_found_it() {
        let node = node_from(
            r#"{"node":{"id":"0.0.2","app":"zenity","role":"text","name":"",
                        "actions":["activate"],"states":["editable"],
                        "labelled":"Street","at":{"x":668,"y":401},
                        "width":168,"height":34,"value":"12 Bishop Street"}}"#,
        )
        .expect("parses");

        assert_eq!(node.labelled.as_deref(), Some("Street"));
        assert_eq!(node.value.as_deref(), Some("12 Bishop Street"));
        assert_eq!(node.at.map(|at| at.x), Some(668));
    }
}
