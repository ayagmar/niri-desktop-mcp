//! `elements`' work, and aiming a pointer tool at an element it listed. niri says where
//! the window is, right when asked; the accessibility bus says what is in it and where
//! relative to the window (see `a11y::model`). Names are the app's data: they are
//! returned, never logged. `actions` acts on a listed element without the pointer.

pub(crate) mod actions;

use std::hash::{BuildHasher as _, RandomState};
use std::sync::OnceLock;

use niri_ipc::Window;
use serde::Serialize;

use crate::a11y::model::{
    self, Extents, Filter, Fresh, LayoutBox, Lineage, Placement, Refused, Unmappable,
};
use crate::a11y::{self, A11y, Capped, ElementRef, Failed, Node, Want};
use crate::coords::LayoutPt;
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri;
use crate::niri::Socket;
use crate::policy::{self, Loaded};

/// What `elements` was asked for.
#[derive(Debug, Clone)]
pub(crate) struct Ask {
    pub(crate) window_id: u64,
    pub(crate) filter: Filter,
    pub(crate) limit: usize,
}

/// What `elements` returns.
#[derive(Debug, Serialize)]
pub(crate) struct Listing {
    pub(crate) window_id: u64,
    pub(crate) elements: Vec<Listed>,
    /// The walk found more matching elements than `limit` and stopped there.
    pub(crate) truncated: bool,
    /// How many accessible objects the walk read.
    pub(crate) walked: usize,
    /// The walk stopped early, so elements further on are missing.
    pub(crate) capped: bool,
    /// Why: `node_cap`, or `budget_exhausted` when the request's three seconds ran out.
    pub(crate) capped_reason: Option<Capped>,
}

/// One element.
#[derive(Debug, Serialize)]
pub(crate) struct Listed {
    /// `elem-<tag>-N` for the pointer tools' `element`, while this server holds the lease.
    pub(crate) element_ref: Option<String>,
    pub(crate) role: &'static str,
    /// The app's text: untrusted data.
    pub(crate) name: String,
    pub(crate) states: Vec<&'static str>,
    pub(crate) actions: Vec<String>,
    pub(crate) layout_box: Option<LayoutBox>,
    pub(crate) unmappable: Option<Unmappable>,
}

/// A window as niri reports it right now, and where its geometry starts in the layout.
#[derive(Debug)]
struct Placed {
    window: Window,
    origin: Option<LayoutPt>,
}

/// niri's window `id` right now, with its place in the layout, from fresh requests.
async fn placed(socket: &Socket, id: u64) -> Result<Option<Placed>, ToolError> {
    let (windows, workspaces, outputs) = tokio::join!(
        niri::windows(socket),
        niri::workspaces(socket),
        niri::outputs(socket)
    );
    let Some(window) = windows?.into_iter().find(|window| window.id == id) else {
        return Ok(None);
    };
    let output = window
        .workspace_id
        .and_then(|workspace| workspaces.ok()?.into_iter().find(|w| w.id == workspace))
        .and_then(|workspace| workspace.output)
        .and_then(|name| outputs.ok()?.remove(&name)?.logical);
    let origin = output.and_then(|output| model::window_origin(&output, &window.layout));
    Ok(Some(Placed { window, origin }))
}

/// Lists the accessible elements of window `ask.window_id`, keeping a ref of each through
/// `remember` while there is a lease.
pub(crate) async fn list(
    socket: &Socket,
    a11y: &A11y,
    policy: &Loaded,
    ask: &Ask,
    mut remember: impl FnMut(ElementRef) -> Option<String>,
) -> Result<Listing, CallError> {
    let placed = placed(socket, ask.window_id).await?.ok_or_else(|| {
        CallError::InvalidArguments(format!("no window with id {}", ask.window_id))
    })?;
    let window = &placed.window;
    if let Some(refused) = policy::refuse_window(policy, window.id, window.app_id.as_deref()) {
        return Err(refused.into());
    }
    let pid = window.pid.ok_or_else(|| {
        a11y::not_accessible(format!("niri knows no process for window {}", window.id))
    })?;
    let request = a11y.request(a11y::BUDGET).await?;
    let app = request.app(pid).await?;
    let frame = request
        .frame(&app, window.layout.window_size, window.title.as_deref())
        .await?;
    let want = Want {
        wanted: ask.limit,
        matches: |node: &Node| listed(&ask.filter, node),
    };
    let walked = request.walk(&app.bus, &frame.node, want).await?;
    let mut matching = walked
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| listed(&ask.filter, node));
    let mut elements = Vec::new();
    for (at, node) in matching.by_ref().take(ask.limit) {
        let placement = Placement {
            states: node.states,
            extents: node.extents.unwrap_or(Extents {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }),
            frame_fits: frame.fits,
            origin: placed.origin,
        };
        let placed_box = model::place(placement);
        let element_ref = remember(ElementRef {
            bus: app.bus.clone(),
            path: node.path.clone(),
            frame: frame.node.path.clone(),
            kept: model::Kept {
                role: node.role,
                window: window.id,
                pid,
                actions: node.actions.clone(),
                lineage: Lineage {
                    chain: walked.lineage(at),
                    name: name_hash(&node.name),
                },
            },
        });
        elements.push(Listed {
            element_ref,
            role: model::role_name(node.role),
            name: node.name.clone(),
            states: node.states.names(),
            actions: node.actions.clone(),
            layout_box: placed_box.ok(),
            unmappable: placed_box.err(),
        });
    }
    Ok(Listing {
        window_id: window.id,
        truncated: matching.next().is_some(),
        elements,
        walked: walked.nodes.len(),
        capped: walked.capped.is_some(),
        capped_reason: walked.capped,
    })
}

/// Whether `elements` lists `node`: showing, matching the filter, and with a name or
/// actions unless a role was asked for.
fn listed(filter: &Filter, node: &Node) -> bool {
    node.states.has(model::State::Showing)
        && (filter.role.is_some() || !node.name.is_empty() || !node.actions.is_empty())
        && filter.matches(model::role_name(node.role), &node.name)
}

/// Where to aim at `element` now: the centre of its box, from niri's geometry and the
/// element as it is now, after the checks of the research report's A5. Nothing is
/// retried.
pub(crate) async fn aim(
    socket: &Socket,
    a11y: &A11y,
    policy: &Loaded,
    element: &ElementRef,
) -> Result<LayoutPt, ToolError> {
    let kept = &element.kept;
    let placed = placed(socket, kept.window)
        .await?
        .filter(|placed| placed.window.pid == Some(kept.pid))
        .ok_or_else(|| stale(&format!("window {} is gone", kept.window)))?;
    let window = &placed.window;
    if let Some(refused) = policy::refuse_window(policy, window.id, window.app_id.as_deref()) {
        return Err(refused);
    }
    let request = a11y.request(a11y::BUDGET).await?;
    identify(&request, element, window).await?;
    let probe = match request.probe(element).await {
        Ok(probe) => probe,
        Err(Failed::Gone(detail)) => return Err(stale(&detail)),
        Err(failed) => return Err(failed.into()),
    };
    let fresh = Fresh {
        role: probe.role,
        placement: Placement {
            states: probe.states,
            extents: probe.extents,
            frame_fits: model::frame_fits(probe.frame, window.layout.window_size),
            origin: placed.origin,
        },
    };
    match model::recheck(kept, fresh) {
        Ok(found) => Ok(found.centre()),
        Err(Refused::Stale(detail)) => Err(stale(&detail)),
        Err(Refused::Unmappable(why)) => Err(unmappable(why)),
    }
}

/// A hash of an element's name, keyed for this process, so a ref can tell whether the name
/// changed without keeping it.
fn name_hash(name: &str) -> u64 {
    static KEYS: OnceLock<RandomState> = OnceLock::new();
    KEYS.get_or_init(RandomState::new).hash_one(name)
}

/// Checks that `element` is still the object `elements` listed, not another that took its
/// path: its application still has niri's `window`, whose accessible frame, found again,
/// is the ref's; each object from that frame down to the element is still at the index
/// it had among its parent's children, as the parent says; and its name is the one it
/// had. Otherwise `element_stale`. Its role is checked by the caller.
pub(crate) async fn identify(
    request: &a11y::Request,
    element: &ElementRef,
    window: &Window,
) -> Result<(), ToolError> {
    let app = request.app(element.kept.pid).await.map_err(|error| {
        if error.name == ErrorName::NotAccessible {
            stale("the application left the accessibility bus")
        } else {
            error
        }
    })?;
    if app.bus != element.bus {
        return Err(stale(
            "the application connected to the accessibility bus again",
        ));
    }
    let frame = request
        .frame(&app, window.layout.window_size, window.title.as_deref())
        .await?;
    if frame.node.path != element.frame {
        return Err(stale(&format!(
            "window {} is another accessible window now",
            window.id
        )));
    }
    let mut parent = element.frame.as_str();
    for (path, index) in &element.kept.lineage.chain {
        let (bus, child) = request
            .child_at(&element.bus, parent, *index)
            .await
            .map_err(gone_is_stale)?;
        if bus != element.bus || child != *path {
            return Err(stale(
                "the element moved in its window, or another element took its place",
            ));
        }
        parent = path;
    }
    let name = request.element_name(element).await.map_err(gone_is_stale)?;
    if name_hash(&name) != element.kept.lineage.name {
        return Err(stale("the element's name changed"));
    }
    Ok(())
}

/// A call on a gone object is `element_stale`; other failures stay as they are.
pub(crate) fn gone_is_stale(failed: Failed) -> ToolError {
    match failed {
        Failed::Gone(detail) => stale(&detail),
        Failed::Refused(error) | Failed::Error(error) => error,
    }
}

fn stale(detail: &str) -> ToolError {
    ToolError::new(
        ErrorName::ElementStale,
        format!("{detail}; call elements again"),
    )
}

/// `element_unmappable`, with the reason first, as `ref_invalid` has it.
pub(crate) fn unmappable(why: Unmappable) -> ToolError {
    let (reason, detail) = match why {
        Unmappable::FrameSizeMismatch => (
            "frame_size_mismatch",
            "the app's accessible window isn't the size niri gives the window, so its coordinates can't be trusted; aim with a screenshot pixel instead",
        ),
        Unmappable::NotShowing => (
            "not_showing",
            "the element or its window isn't showing; bring it into view and call elements again",
        ),
        Unmappable::Empty => ("empty", "the element has no area"),
        Unmappable::OutsideScreenshot => (
            "outside_screenshot",
            "the element's centre is outside the screenshot; take a screenshot that shows it",
        ),
    };
    ToolError::new(ErrorName::ElementUnmappable, format!("{reason}: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a11y::model::States;

    const SHOWING: u32 = (1 << 25) | (1 << 30);
    const BUTTON: u32 = 43;
    const FILLER: u32 = 20;

    fn node(role: u32, name: &str, showing: bool, actions: &[&str]) -> Node {
        Node {
            path: "/n".to_owned(),
            role,
            name: name.to_owned(),
            states: States::from_words(&[if showing { SHOWING } else { 0 }]),
            extents: None,
            actions: actions.iter().map(|&action| action.to_owned()).collect(),
            children: Vec::new(),
        }
    }

    #[test]
    fn only_showing_elements_that_pass_every_filter_count_towards_the_limit() {
        let save = Filter::new(Some("button".to_owned()), Some("SAVE"));
        assert!(listed(&save, &node(BUTTON, "Save as", true, &[])));
        // Other buttons don't use up the limit before the one asked for.
        assert!(!listed(&save, &node(BUTTON, "Open", true, &["click"])));
        assert!(!listed(&save, &node(BUTTON, "Save", false, &[])));
        assert!(!listed(&save, &node(FILLER, "Save", true, &[])));

        let any = Filter::default();
        assert!(listed(&any, &node(FILLER, "", true, &["click"])));
        assert!(listed(&any, &node(FILLER, "Title", true, &[])));
        assert!(!listed(&any, &node(FILLER, "", true, &[])));
        let fillers = Filter::new(Some("filler".to_owned()), None);
        assert!(listed(&fillers, &node(FILLER, "", true, &[])));
    }
}
