use slotmap::SlotMap;

use crate::surface::{Surface, SurfaceId};

#[derive(Default)]
pub struct State {
    pub surfaces: SlotMap<SurfaceId, Surface>,
}
