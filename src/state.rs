use slotmap::SlotMap;

use crate::{
    seat::Seat,
    surface::{Surface, SurfaceId},
};

#[derive(Default)]
pub struct State {
    pub surfaces: SlotMap<SurfaceId, Surface>,
    pub seat: Option<Seat>,
    pub xkb_ctx: kbvm::xkb::Context,
}
