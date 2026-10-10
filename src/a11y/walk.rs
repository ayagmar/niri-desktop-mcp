//! The bounded walk of one window's accessible tree, over any source of nodes. It visits
//! nodes depth first in document order, goes into a node's children only while the node
//! is showing, and stops after a fixed number of nodes, once it has found more matching
//! nodes than were asked for, or when the request's budget runs out, so a huge or hidden
//! tree can't hold the request.

use std::future::Future;

use serde::Serialize;

use super::model::{Extents, States};
use crate::error::{ErrorName, ToolError};

/// One accessible object, as the walk read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) path: String,
    pub(crate) role: u32,
    /// Data from the app: never logged.
    pub(crate) name: String,
    pub(crate) states: States,
    /// None without a Component interface.
    pub(crate) extents: Option<Extents>,
    pub(crate) actions: Vec<String>,
    /// Its children in the same application.
    pub(crate) children: Vec<Child>,
}

/// A child of a node: its object path, and its index among all of the node's children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Child {
    pub(crate) index: i32,
    pub(crate) path: String,
}

/// Where the walk reads nodes from.
pub(crate) trait Source: Sync {
    /// The node at `path`, or `None` when it no longer exists, as when a widget went away
    /// during the walk.
    fn node(&self, path: &str) -> impl Future<Output = Result<Option<Node>, ToolError>> + Send;

    /// Whether the request's whole budget is used up, as opposed to one call's deadline.
    fn spent(&self) -> bool;
}

/// Why a walk ended with nodes left to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Capped {
    /// It read as many nodes as it may.
    NodeCap,
    /// The request's whole budget ran out.
    BudgetExhausted,
}

/// The nodes under a root, in document order, without the root.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Walked {
    pub(crate) nodes: Vec<Node>,
    /// Where each node of `nodes` sits: its parent among them, none under the root, and
    /// its index among the parent's children.
    places: Vec<(Option<usize>, i32)>,
    /// Set when the walk stopped early with nodes left that might have matched.
    pub(crate) capped: Option<Capped>,
}

impl Walked {
    /// The path and index in its parent of each node from the root's child down to the
    /// node at `at`.
    pub(crate) fn lineage(&self, at: usize) -> Vec<(String, i32)> {
        let mut chain = Vec::new();
        let mut next = Some(at);
        while let Some(this) = next {
            let (Some(node), Some((parent, index))) = (self.nodes.get(this), self.places.get(this))
            else {
                break;
            };
            chain.push((node.path.clone(), *index));
            next = *parent;
        }
        chain.reverse();
        chain
    }
}

/// What the walk is looking for: nodes for which `matches` holds, and how many of them
/// are wanted. The walk stops once it found one more than `wanted`, which is enough to
/// know there are more.
pub(crate) struct Want<F> {
    pub(crate) wanted: usize,
    pub(crate) matches: F,
}

/// Reads at most `cap` nodes under `root`, depth first, going into the children of
/// showing nodes only, matching or not, and stops early once `want` has more matches than
/// it asked for. A node that is gone by the time it is read is skipped. A deadline once
/// the request's budget is spent ends the walk with the nodes read so far,
/// `BudgetExhausted`. One call's own deadline, as for a hung app, and any other failure
/// end the walk with that error.
pub(crate) async fn walk<F: Fn(&Node) -> bool + Sync>(
    source: &impl Source,
    root: &Node,
    cap: usize,
    want: Want<F>,
) -> Result<Walked, ToolError> {
    let mut walked = Walked::default();
    let mut found = 0;
    let mut pending: Vec<(Option<usize>, Child)> = root
        .children
        .iter()
        .rev()
        .map(|child| (None, child.clone()))
        .collect();
    while let Some((parent, child)) = pending.pop() {
        if walked.nodes.len() == cap {
            walked.capped = Some(Capped::NodeCap);
            break;
        }
        let node = match source.node(&child.path).await {
            Ok(Some(node)) => node,
            Ok(None) => continue,
            Err(error) if error.name == ErrorName::DeadlineExceeded && source.spent() => {
                walked.capped = Some(Capped::BudgetExhausted);
                break;
            }
            Err(error) => return Err(error),
        };
        let at = walked.nodes.len();
        if node.states.has(super::model::State::Showing) {
            pending.extend(
                node.children
                    .iter()
                    .rev()
                    .map(|grandchild| (Some(at), grandchild.clone())),
            );
        }
        found += usize::from((want.matches)(&node));
        walked.nodes.push(node);
        walked.places.push((parent, child.index));
        if found > want.wanted {
            break;
        }
    }
    Ok(walked)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::a11y::model::State;

    const SHOWING: u32 = (1 << 25) | (1 << 30);

    /// An in-memory tree: path → (showing, children). Missing paths are gone. `/late`
    /// times out with the request's budget spent, `/hung` times out on its own call with
    /// budget left, as a hung app does, and `/lost` fails as a lost bus connection does.
    struct Tree {
        nodes: BTreeMap<&'static str, (bool, Vec<&'static str>)>,
        spent: AtomicBool,
    }

    impl Tree {
        fn new<const N: usize>(nodes: [(&'static str, (bool, Vec<&'static str>)); N]) -> Self {
            Self {
                nodes: BTreeMap::from(nodes),
                spent: AtomicBool::new(false),
            }
        }
    }

    impl Source for Tree {
        fn node(&self, path: &str) -> impl Future<Output = Result<Option<Node>, ToolError>> + Send {
            let failure = match path {
                "/late" => {
                    self.spent.store(true, Ordering::Relaxed);
                    Some(ErrorName::DeadlineExceeded)
                }
                "/hung" => Some(ErrorName::DeadlineExceeded),
                "/lost" => Some(ErrorName::UpstreamError),
                _ => None,
            };
            if let Some(name) = failure {
                return std::future::ready(Err(ToolError::new(name, path)));
            }
            std::future::ready(Ok(self.nodes.get(path).map(|(showing, children)| Node {
                path: path.to_owned(),
                role: 43,
                name: String::new(),
                states: States::from_words(&[if *showing { SHOWING } else { 0 }]),
                extents: None,
                actions: Vec::new(),
                children: kids(children),
            })))
        }

        fn spent(&self) -> bool {
            self.spent.load(Ordering::Relaxed)
        }
    }

    /// Children at their index in the list.
    fn kids(paths: &[&str]) -> Vec<Child> {
        (0..)
            .zip(paths)
            .map(|(index, &path)| Child {
                index,
                path: path.to_owned(),
            })
            .collect()
    }

    fn root(children: &[&str]) -> Node {
        Node {
            path: "/root".to_owned(),
            role: 23,
            name: String::new(),
            states: States::from_words(&[SHOWING]),
            extents: None,
            actions: Vec::new(),
            children: kids(children),
        }
    }

    fn paths(walked: &Walked) -> Vec<&str> {
        walked.nodes.iter().map(|node| node.path.as_str()).collect()
    }

    fn everything() -> Want<fn(&Node) -> bool> {
        Want {
            wanted: usize::MAX,
            matches: |_| true,
        }
    }

    /// Wants `wanted` showing nodes whose path starts with `prefix`.
    fn showing_under(prefix: &'static str, wanted: usize) -> Want<impl Fn(&Node) -> bool + Sync> {
        Want {
            wanted,
            matches: move |node: &Node| {
                node.states.has(State::Showing) && node.path.starts_with(prefix)
            },
        }
    }

    #[tokio::test]
    async fn visits_depth_first_in_document_order_and_skips_hidden_subtrees() {
        let tree = Tree::new([
            ("/a", (true, vec!["/a/1", "/a/2"])),
            ("/a/1", (true, vec![])),
            ("/a/2", (true, vec![])),
            ("/hidden", (false, vec!["/hidden/1"])),
            ("/hidden/1", (true, vec![])),
            ("/b", (true, vec![])),
        ]);
        let tree_root = root(&["/a", "/gone", "/hidden", "/b"]);
        let walked = walk(&tree, &tree_root, 2000, everything()).await.unwrap();
        assert_eq!(paths(&walked), ["/a", "/a/1", "/a/2", "/hidden", "/b"]);
        assert_eq!(walked.capped, None);
    }

    #[tokio::test]
    async fn a_nodes_lineage_is_each_ancestors_place_from_the_roots_child_down() {
        let tree = Tree::new([
            ("/a", (true, vec!["/a/1", "/a/2"])),
            ("/a/1", (true, vec![])),
            ("/a/2", (true, vec!["/a/2/x"])),
            ("/a/2/x", (true, vec![])),
            ("/b", (true, vec![])),
        ]);
        let walked = walk(&tree, &root(&["/gone", "/a", "/b"]), 2000, everything())
            .await
            .unwrap();
        let at = |path: &str| walked.nodes.iter().position(|node| node.path == path);
        let place = |path: &str, index| (path.to_owned(), index);
        assert_eq!(
            walked.lineage(at("/a/2/x").unwrap()),
            [place("/a", 1), place("/a/2", 1), place("/a/2/x", 0)]
        );
        // A gone sibling keeps its place in the parent's list.
        assert_eq!(walked.lineage(at("/b").unwrap()), [place("/b", 2)]);
    }

    #[tokio::test]
    async fn stops_at_the_node_cap_however_few_match() {
        let tree = Tree::new([
            ("/a", (true, vec![])),
            ("/b", (true, vec![])),
            ("/c", (true, vec![])),
        ]);
        let walked = walk(&tree, &root(&["/a", "/b", "/c"]), 2, showing_under("/x", 1))
            .await
            .unwrap();
        assert_eq!(paths(&walked), ["/a", "/b"]);
        assert_eq!(walked.capped, Some(Capped::NodeCap));
        let exact = walk(&tree, &root(&["/a", "/b"]), 2, everything())
            .await
            .unwrap();
        assert_eq!(exact.capped, None);
    }

    #[tokio::test]
    async fn finds_matches_under_parents_that_dont_match_and_stops_past_the_limit() {
        let tree = Tree::new([
            ("/a", (true, vec!["/a/1", "/b/1"])),
            ("/a/1", (true, vec![])),
            ("/b/1", (true, vec![])),
            ("/hidden", (false, vec!["/b/hidden"])),
            ("/b/hidden", (true, vec![])),
            ("/b/2", (true, vec![])),
            ("/b/3", (true, vec![])),
            ("/b/4", (true, vec![])),
        ]);
        let tree_root = root(&["/a", "/hidden", "/b/2", "/b/3", "/b/4"]);
        let walked = walk(&tree, &tree_root, 2000, showing_under("/b", 2))
            .await
            .unwrap();
        // /b/1 sits under /a, which doesn't match; /b/hidden is never read.
        assert_eq!(
            paths(&walked),
            ["/a", "/a/1", "/b/1", "/hidden", "/b/2", "/b/3"]
        );
        assert_eq!(walked.capped, None);
    }

    #[tokio::test]
    async fn exactly_the_limit_walks_the_whole_tree() {
        let tree = Tree::new([
            ("/b/1", (true, vec![])),
            ("/a", (true, vec![])),
            ("/b/2", (true, vec![])),
        ]);
        let tree_root = root(&["/b/1", "/a", "/b/2"]);
        let walked = walk(&tree, &tree_root, 2000, showing_under("/b", 2))
            .await
            .unwrap();
        assert_eq!(paths(&walked), ["/b/1", "/a", "/b/2"]);
        assert_eq!(walked.capped, None);
    }

    #[tokio::test]
    async fn a_spent_budget_keeps_what_was_read_before_or_after_a_match() {
        let early = Tree::new([("/b/1", (true, vec![])), ("/a", (true, vec![]))]);
        let before = walk(
            &early,
            &root(&["/late", "/b/1"]),
            2000,
            showing_under("/b", 5),
        )
        .await
        .unwrap();
        assert_eq!(paths(&before), Vec::<&str>::new());
        assert_eq!(before.capped, Some(Capped::BudgetExhausted));
        let late = Tree::new([("/b/1", (true, vec![])), ("/a", (true, vec![]))]);
        let tree_root = root(&["/b/1", "/a", "/late"]);
        let after = walk(&late, &tree_root, 2000, showing_under("/b", 5))
            .await
            .unwrap();
        assert_eq!(paths(&after), ["/b/1", "/a"]);
        assert_eq!(after.capped, Some(Capped::BudgetExhausted));
    }

    #[tokio::test]
    async fn one_calls_deadline_and_a_lost_bus_are_errors_even_after_nodes() {
        let tree = Tree::new([("/a", (true, vec![]))]);
        for (failing, name) in [
            ("/hung", ErrorName::DeadlineExceeded),
            ("/lost", ErrorName::UpstreamError),
        ] {
            let error = walk(&tree, &root(&["/a", failing]), 2000, everything())
                .await
                .unwrap_err();
            assert_eq!(error.name, name, "{failing}");
        }
    }
}
