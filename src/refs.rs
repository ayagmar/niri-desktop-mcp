//! Screenshot refs (plan §8): what each screenshot taken under the lease captured, kept in
//! memory so a pointer tool can map a pixel of that image back to the layout. Element refs
//! keep the accessible elements `elements` listed under the lease, so a pointer tool can
//! aim at one. Refs belong to one lease: taking or giving up the lease drops them all, and
//! an id is never issued twice by one server, so a ref from an earlier lease is simply
//! unknown. Every id carries the server process's tag, so neither is a ref from another
//! process, such as a server that ran before this one.

use std::collections::{BTreeMap, VecDeque};
use std::hash::BuildHasher as _;
use std::sync::OnceLock;
use std::time::Duration;

use niri_ipc::{LogicalOutput, Output};
use tokio::time::Instant;

use crate::a11y::ElementRef;
use crate::a11y::model::NameHash;
use crate::coords::{self, Capture, ImagePx, LayoutPt, ProtocolPt};
use crate::error::{ErrorName, ToolError};
use crate::observe::{Rect, Screenshot};

/// How many refs a lease keeps; older ones are dropped first.
const KEPT: usize = 64;
/// How many element refs a lease keeps; older ones are dropped first.
const KEPT_ELEMENTS: usize = 1000;
/// How long a ref stays usable (plan §6).
const MAX_AGE: Duration = Duration::from_secs(60);

/// One screenshot, as a pointer tool needs it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shot {
    pub(crate) output: String,
    /// The output as niri described it at capture.
    pub(crate) geometry: LogicalOutput,
    pub(crate) motion_geometry: Option<LogicalOutput>,
    /// The captured rectangle in layout coordinates.
    pub(crate) captured: Rect,
    /// Image pixels per logical pixel.
    pub(crate) scale: f64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// When the capture started.
    pub(crate) taken: Instant,
    /// The event stream's connection at capture: a disconnect drops every ref.
    pub(crate) connection: u64,
}

impl Shot {
    /// What a pointer tool needs from `shot`, whose capture started at `taken` while the
    /// event stream was on `connection`.
    pub(crate) fn of(shot: &Screenshot, taken: Instant, connection: u64) -> Self {
        let metadata = &shot.metadata;
        Self {
            output: metadata.output.clone(),
            geometry: shot.geometry,
            motion_geometry: shot.motion_geometry,
            captured: metadata.captured,
            scale: metadata.scale,
            width: metadata.width,
            height: metadata.height,
            taken,
            connection,
        }
    }
}

impl Shot {
    /// Where layout point `point` is now, as a `motion_absolute` on the captured output,
    /// provided the ref is fresh, the output is unchanged in `outputs`, the event stream
    /// is still on the capture's `connection`, and the point is inside the captured
    /// rectangle.
    pub(crate) fn aim_at(
        &self,
        point: LayoutPt,
        now: Instant,
        outputs: &BTreeMap<String, Output>,
        connection: Option<u64>,
    ) -> Result<ProtocolPt, ToolError> {
        let motion = self.check(now, outputs, connection)?;
        if !self.contains(point) {
            return Err(invalid(
                "out_of_bounds",
                &format!(
                    "({:.2}, {:.2}) is outside the screenshot's {:?}",
                    point.x, point.y, self.captured
                ),
            ));
        }
        self.encode(point, &motion)
    }

    /// Whether `point` lies in the captured rectangle.
    pub(crate) fn contains(&self, point: LayoutPt) -> bool {
        let Rect {
            x,
            y,
            width,
            height,
        } = self.captured;
        let (x, y) = (f64::from(x), f64::from(y));
        (x..x + f64::from(width)).contains(&point.x)
            && (y..y + f64::from(height)).contains(&point.y)
    }

    /// The layout point a pixel of this image targets, its centre, if the pixel is inside
    /// the image.
    pub(crate) fn layout_of(&self, pixel: ImagePx) -> Result<LayoutPt, ToolError> {
        if pixel.x >= self.width || pixel.y >= self.height {
            return Err(invalid(
                "out_of_bounds",
                &format!(
                    "pixel ({}, {}) is outside the {}x{} image",
                    pixel.x, pixel.y, self.width, self.height
                ),
            ));
        }
        let capture = Capture {
            origin: LayoutPt {
                x: f64::from(self.captured.x),
                y: f64::from(self.captured.y),
            },
            scale: self.scale,
        };
        Ok(coords::image_to_layout(pixel, capture))
    }

    /// The ref is fresh, the event stream is still on the capture's connection, and the
    /// output is unchanged: the pointer space to encode in.
    pub(crate) fn check(
        &self,
        now: Instant,
        outputs: &BTreeMap<String, Output>,
        connection: Option<u64>,
    ) -> Result<LogicalOutput, ToolError> {
        let age = now.saturating_duration_since(self.taken);
        if age > MAX_AGE {
            return Err(invalid(
                "expired",
                &format!("the screenshot is {} s old; take a new one", age.as_secs()),
            ));
        }
        if connection != Some(self.connection) {
            return Err(invalid(
                "unknown_ref",
                "niri's event stream reconnected since the screenshot; take a new one",
            ));
        }
        let now_geometry = outputs.get(&self.output).and_then(|output| output.logical);
        if now_geometry != Some(self.geometry) {
            return Err(invalid(
                "output_changed",
                &format!(
                    "output {} was {:?} at the screenshot and is {now_geometry:?} now; take a new one",
                    self.output, self.geometry
                ),
            ));
        }
        let motion = self.motion_geometry.ok_or_else(|| {
            invalid(
                "unknown_geometry",
                "the screenshot output has no usable physical mode; take a new screenshot",
            )
        })?;
        let now_motion = outputs.get(&self.output).and_then(coords::motion_geometry);
        if now_motion != Some(motion) {
            return Err(invalid(
                "output_changed",
                "the output's pointer space changed since capture; take a new screenshot",
            ));
        }
        Ok(motion)
    }

    fn encode(&self, point: LayoutPt, motion: &LogicalOutput) -> Result<ProtocolPt, ToolError> {
        coords::checked_encode(point, motion).map_err(|distance| {
            invalid(
                "out_of_bounds",
                &format!(
                    "({:.2}, {:.2}) maps {distance:.3} px off output {}",
                    point.x, point.y, self.output
                ),
            )
        })
    }
}

/// `ref_invalid`, with the reason first.
pub(crate) fn invalid(reason: &str, detail: &str) -> ToolError {
    ToolError::new(ErrorName::RefInvalid, format!("{reason}: {detail}"))
}

/// The refs of the current lease.
#[derive(Debug)]
pub(crate) struct Refs {
    /// In every id, between the kind and the number.
    tag: String,
    /// Leases seen, counted from 1, and refs issued: ids are never reused.
    leases: u64,
    issued: u64,
    /// The lease the kept refs belong to, while one is held.
    lease: Option<u64>,
    shots: VecDeque<(u64, Shot)>,
    elements_issued: u64,
    elements: VecDeque<(u64, ElementRef)>,
}

impl Default for Refs {
    /// Refs whose ids carry this process's tag.
    fn default() -> Self {
        Self::new(process_tag().to_owned())
    }
}

/// Eight hex digits, random for each process.
fn process_tag() -> &'static str {
    static TAG: OnceLock<String> = OnceLock::new();
    TAG.get_or_init(|| {
        // The standard library seeds its hash keys from the system's randomness.
        let random = std::hash::RandomState::new().hash_one(std::process::id());
        format!("{:08x}", random & 0xFFFF_FFFF)
    })
}

impl Refs {
    /// No refs yet, with `tag` in every id.
    pub(crate) const fn new(tag: String) -> Self {
        Self {
            tag,
            leases: 0,
            issued: 0,
            lease: None,
            shots: VecDeque::new(),
            elements_issued: 0,
            elements: VecDeque::new(),
        }
    }

    /// The number in `id`, if it is an id of `kind` with this tag.
    fn number(&self, id: &str, kind: &str) -> Option<u64> {
        let rest = id.strip_prefix(kind)?.strip_prefix('-')?;
        rest.strip_prefix(self.tag.as_str())?
            .strip_prefix('-')?
            .parse()
            .ok()
    }

    /// A new lease: earlier refs are dropped.
    pub(crate) fn start(&mut self) {
        self.leases += 1;
        self.lease = Some(self.leases);
        self.shots.clear();
        self.elements.clear();
    }

    /// The lease is gone, and with it its refs.
    pub(crate) fn end(&mut self) {
        self.lease = None;
        self.shots.clear();
        self.elements.clear();
    }

    /// The lease a screenshot starting now would belong to.
    pub(crate) const fn lease(&self) -> Option<u64> {
        self.lease
    }

    /// Keeps `shot` and returns its id, if `lease` is still the current one.
    pub(crate) fn insert(&mut self, lease: u64, shot: Shot) -> Option<String> {
        if self.lease != Some(lease) {
            return None;
        }
        self.issued += 1;
        if self.shots.len() == KEPT {
            self.shots.pop_front();
        }
        self.shots.push_back((self.issued, shot));
        Some(format!("shot-{}-{}", self.tag, self.issued))
    }

    /// Keeps `element` and returns its id, `elem-<tag>-N`, if `lease` is still the current
    /// one.
    pub(crate) fn insert_element(&mut self, lease: u64, element: ElementRef) -> Option<String> {
        if self.lease != Some(lease) {
            return None;
        }
        self.elements_issued += 1;
        if self.elements.len() == KEPT_ELEMENTS {
            self.elements.pop_front();
        }
        self.elements.push_back((self.elements_issued, element));
        Some(format!("elem-{}-{}", self.tag, self.elements_issued))
    }

    /// Keeps `name` as the name of the element the ref `id` names, as an element action
    /// read it after acting, if the current lease kept that ref.
    pub(crate) fn rename_element(&mut self, id: &str, name: NameHash) {
        let wanted = self.number(id, "elem");
        let element = self
            .elements
            .iter_mut()
            .find(|(issued, _)| Some(*issued) == wanted)
            .and_then(|(_, element)| element.kept.lineage.last_mut());
        if let Some(own) = element {
            own.name = name;
        }
    }

    /// The element ref named `id`, if the current lease kept it.
    pub(crate) fn element(&self, id: &str) -> Result<ElementRef, ToolError> {
        let wanted = self.number(id, "elem");
        self.elements
            .iter()
            .find(|(issued, _)| Some(*issued) == wanted)
            .map(|(_, element)| element.clone())
            .ok_or_else(|| {
                ToolError::new(
                    ErrorName::ElementStale,
                    format!(
                        "{id:?} isn't an element listed under this lease; call elements after acquire_desktop"
                    ),
                )
            })
    }

    /// The ref named `id`, if the current lease kept it.
    pub(crate) fn get(&self, id: &str) -> Result<Shot, ToolError> {
        let wanted = self.number(id, "shot");
        self.shots
            .iter()
            .find(|(issued, _)| Some(*issued) == wanted)
            .map(|(_, shot)| shot.clone())
            .ok_or_else(|| {
                invalid(
                    "unknown_ref",
                    &format!(
                        "{id:?} isn't a screenshot of this lease; take a screenshot after acquire_desktop"
                    ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::shot;

    fn outputs(geometry: LogicalOutput) -> BTreeMap<String, Output> {
        let mode = crate::test_support::output_mode::from_logical(
            geometry.width,
            geometry.height,
            geometry.scale,
        );
        let output = serde_json::from_value(serde_json::json!({
            "name": "winit", "make": "", "model": "", "serial": null, "physical_size": null,
            "modes": [mode], "current_mode": 0, "is_custom_mode": false,
            "vrr_supported": false, "vrr_enabled": false, "logical": geometry
        }))
        .unwrap();
        BTreeMap::from([("winit".to_owned(), output)])
    }

    /// A pixel of `shot` aimed as the pointer tools aim it.
    fn aim(
        shot: &Shot,
        pixel: ImagePx,
        now: Instant,
        outputs: &BTreeMap<String, Output>,
        connection: Option<u64>,
    ) -> Result<ProtocolPt, ToolError> {
        shot.check(now, outputs, connection)?;
        shot.aim_at(shot.layout_of(pixel)?, now, outputs, connection)
    }

    fn reason(result: Result<ProtocolPt, ToolError>) -> String {
        let error = result.unwrap_err();
        assert_eq!(error.name, ErrorName::RefInvalid);
        error.detail.split(':').next().unwrap().to_owned()
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_or_changed_physical_mode_never_reuses_a_pointer_mapping() {
        let mut shot = shot();
        let mut now = outputs(shot.geometry);
        let centre = ImagePx { x: 720, y: 540 };
        now.get_mut("winit")
            .unwrap()
            .modes
            .first_mut()
            .unwrap()
            .width += 1;
        assert_eq!(
            reason(aim(&shot, centre, Instant::now(), &now, Some(1))),
            "output_changed"
        );
        now.get_mut("winit").unwrap().modes.clear();
        assert_eq!(
            reason(aim(&shot, centre, Instant::now(), &now, Some(1))),
            "output_changed"
        );
        shot.motion_geometry = None;
        assert_eq!(
            reason(aim(&shot, centre, Instant::now(), &now, Some(1))),
            "unknown_geometry"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn fractional_output_motion_uses_niris_ceiled_space_not_its_truncated_ipc_size() {
        let mut shot = shot();
        shot.geometry.width = 853;
        shot.geometry.height = 480;
        shot.captured.width = 853;
        shot.captured.height = 480;
        shot.width = 1279;
        shot.height = 720;
        let mut now = outputs(shot.geometry);
        let output = now.get_mut("winit").unwrap();
        output.modes = vec![niri_ipc::Mode {
            width: 1280,
            height: 720,
            refresh_rate: 60000,
            is_preferred: true,
        }];
        output.current_mode = Some(0);
        shot.motion_geometry = coords::motion_geometry(output);
        let pixel = ImagePx { x: 600, y: 300 };
        let request = aim(&shot, pixel, Instant::now(), &now, Some(1)).unwrap();
        // niri 26.04 uses Smithay's ceiled output space: ceil(1280 / 1.5) = 854.
        let landed_x = f64::from(request.x) * 854.0 / f64::from(request.x_extent);
        let target_x = 600.5 / 1.5;
        assert!(
            (landed_x - target_x).abs() <= 0.002,
            "landed {landed_x}, target {target_x}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_ref_aims_only_while_fresh_unchanged_and_inside_its_image() {
        let shot = shot();
        let now = outputs(shot.geometry);
        let centre = ImagePx { x: 720, y: 540 };
        // The image's centre at scale 1.5 is the output's, mirrored vertically by
        // Flipped180 into the untransformed space: (480, 360) either way.
        assert_eq!(
            aim(&shot, centre, Instant::now(), &now, Some(1)),
            Ok(ProtocolPt {
                x: 480_333,
                y: 359_667,
                x_extent: 960_000,
                y_extent: 720_000
            })
        );
        let later = Instant::now() + MAX_AGE + Duration::from_secs(1);
        assert_eq!(reason(aim(&shot, centre, later, &now, Some(1))), "expired");
        assert_eq!(
            reason(aim(&shot, centre, Instant::now(), &now, Some(2))),
            "unknown_ref"
        );
        let moved = outputs(LogicalOutput {
            scale: 1.0,
            ..shot.geometry
        });
        assert_eq!(
            reason(aim(&shot, centre, Instant::now(), &moved, Some(1))),
            "output_changed"
        );
        assert_eq!(
            reason(aim(
                &shot,
                centre,
                Instant::now(),
                &BTreeMap::new(),
                Some(1)
            )),
            "output_changed"
        );
        for pixel in [ImagePx { x: 1440, y: 0 }, ImagePx { x: 0, y: 1080 }] {
            assert_eq!(
                reason(aim(&shot, pixel, Instant::now(), &now, Some(1))),
                "out_of_bounds"
            );
        }
        assert!(
            aim(
                &shot,
                ImagePx { x: 1439, y: 1079 },
                Instant::now(),
                &now,
                Some(1)
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn an_id_from_another_process_names_nothing_here() {
        let mut earlier = Refs::new("0a1b2c3d".to_owned());
        let mut now = Refs::new("4e5f6a7b".to_owned());
        earlier.start();
        now.start();
        let old = earlier.insert(earlier.lease().unwrap(), shot()).unwrap();
        let new = now.insert(now.lease().unwrap(), shot()).unwrap();
        assert_ne!(old, new);
        assert!(now.get(&new).is_ok());
        let error = now.get(&old).unwrap_err();
        assert!(error.detail.starts_with("unknown_ref"), "{}", error.detail);
    }

    #[test]
    fn element_refs_belong_to_one_lease_and_keep_the_latest_thousand() {
        let element = || ElementRef {
            bus: ":1.7".to_owned(),
            path: "/org/a11y/atspi/accessible/9".to_owned(),
            frame: "/org/a11y/atspi/accessible/1".to_owned(),
            kept: crate::a11y::model::Kept {
                role: 43,
                window: 3,
                pid: 4711,
                actions: Vec::new(),
                lineage: Vec::new(),
            },
        };
        let stale = |refs: &Refs, id: &str| refs.element(id).unwrap_err().name;
        let mut refs = Refs::new("t".to_owned());
        assert_eq!(refs.insert_element(1, element()), None);
        refs.start();
        let first = refs.lease().unwrap();
        assert_eq!(
            refs.insert_element(first, element()).as_deref(),
            Some("elem-t-1")
        );
        assert_eq!(refs.element("elem-t-1"), Ok(element()));
        refs.start();
        let second = refs.lease().unwrap();
        assert_eq!(stale(&refs, "elem-t-1"), ErrorName::ElementStale);
        assert_eq!(refs.insert_element(first, element()), None);
        for _ in 0..=KEPT_ELEMENTS {
            refs.insert_element(second, element());
        }
        assert_eq!(stale(&refs, "elem-t-2"), ErrorName::ElementStale);
        assert!(refs.element("elem-t-3").is_ok());
        assert_eq!(stale(&refs, "shot-t-3"), ErrorName::ElementStale);
    }

    #[tokio::test]
    async fn refs_belong_to_one_lease_and_ids_are_never_reused() {
        let mut refs = Refs::new("t".to_owned());
        assert_eq!(refs.lease(), None);
        assert_eq!(refs.insert(1, shot()), None);
        refs.start();
        let first = refs.lease().unwrap();
        assert_eq!(refs.insert(first, shot()).as_deref(), Some("shot-t-1"));
        refs.end();
        // A capture that started under the old lease isn't kept.
        assert_eq!(refs.insert(first, shot()), None);
        refs.start();
        let second = refs.lease().unwrap();
        assert_ne!(first, second);
        assert_eq!(refs.insert(first, shot()), None);
        assert_eq!(refs.insert(second, shot()).as_deref(), Some("shot-t-2"));
        assert_eq!(refs.shots.len(), 1);
        assert!(refs.get("shot-t-2").is_ok());
        for unknown in ["shot-t-1", "shot-t-9", "shot-2", "2", "shot-t-x"] {
            assert_eq!(
                reason(refs.get(unknown).map(|_| ProtocolPt {
                    x: 0,
                    y: 0,
                    x_extent: 1,
                    y_extent: 1
                })),
                "unknown_ref"
            );
        }
        for _ in 0..KEPT {
            refs.insert(second, shot());
        }
        assert_eq!(refs.shots.len(), KEPT);
        assert_eq!(refs.shots.front().map(|(id, _)| *id), Some(3));
    }
}
