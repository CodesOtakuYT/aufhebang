//! Snake, drawn with one subsurface per tile.
//!
//! Only the tiles that exist get a surface: the snake's body and the food. A
//! surface's position is fixed at creation — there is no way to move one — so
//! each step removes the tail's surface and creates one for the new head.
//!
//! The game runs on a timer. That is the whole reason this example is async:
//! `Display::dispatch` sleeps inside the compositor's socket, so a blocking
//! loop has no way to wake up on its own and nothing can advance unless the user
//! presses a key. The library has no timer and no runtime in it — it hands over
//! the socket and the pieces of the read protocol, and the event loop here is
//! ordinary `tokio::select!` over that socket and a `tokio::time` interval.

use std::{
    os::fd::{AsRawFd, RawFd},
    time::Duration,
};

use anyhow::Result;
use aufhebung::{
    color::Color,
    display::Display,
    state::{Event, SeatEvent, SurfaceEvent},
    surface::{SurfaceId, SurfaceInfo, SurfaceRole},
};
use rand::Rng as _;
use tokio::{io::unix::AsyncFd, time::MissedTickBehavior};
use wayland_client::protocol::wl_buffer::WlBuffer;

/// Size of the playing field, in cells.
const GRID: i32 = 20;

/// Cell size used for the initial window, replaced by the compositor's choice.
const CELL: i32 = 24;

/// How long the snake waits between steps.
const STEP: Duration = Duration::from_millis(180);

/// `AsyncFd` wants a handle it can poll, and it has to own it. The socket
/// belongs to the `Display`, which outlives the loop, so a newtype over the raw
/// fd is enough — nothing can close it out from under us while we hold one.
struct Socket(RawFd);

impl AsRawFd for Socket {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pos(i32, i32);

impl Pos {
    fn step(self, dir: Pos) -> Pos {
        Pos(self.0 + dir.0, self.1 + dir.1)
    }

    fn opposite(self) -> Pos {
        Pos(-self.0, -self.1)
    }
}

fn inside(p: Pos) -> bool {
    (0..GRID).contains(&p.0) && (0..GRID).contains(&p.1)
}

#[derive(Clone, Copy)]
enum Ink {
    Body,
    Head,
    Food,
}

/// Just enough randomness to scatter the food.
impl Game {
    fn cell(&mut self) -> Pos {
        Pos(
            self.rng.random_range(0..GRID),
            self.rng.random_range(0..GRID),
        )
    }
}

struct Tile {
    pos: Pos,
    id: SurfaceId,
}

struct Inks {
    body: WlBuffer,
    head: WlBuffer,
    food: WlBuffer,
}

struct Game {
    window: SurfaceId,
    bg: WlBuffer,
    inks: Inks,
    /// The real cell size, decided when the compositor first configures us.
    cell: i32,
    body: Vec<Tile>,
    dir: Pos,
    wanted: Pos,
    food: Option<Tile>,
    score: usize,
    over: bool,
    rng: rand::rngs::ThreadRng,
}

impl Game {
    fn new(window: SurfaceId, bg: WlBuffer, inks: Inks) -> Self {
        Self {
            window,
            bg,
            inks,
            cell: 0,
            body: Vec::new(),
            dir: Pos(0, 1),
            wanted: Pos(0, 1),
            food: None,
            score: 0,
            over: false,
            rng: rand::rng(),
        }
    }

    fn ink(&self, ink: Ink) -> &WlBuffer {
        match ink {
            Ink::Body => &self.inks.body,
            Ink::Head => &self.inks.head,
            Ink::Food => &self.inks.food,
        }
    }

    /// Put down one tile and fill it.
    fn spawn(&mut self, display: &mut Display, pos: Pos, ink: Ink) -> SurfaceId {
        let id = display
            .add_surface(SurfaceInfo {
                width: self.cell,
                height: self.cell,
                role: SurfaceRole::Subsurface {
                    parent: self.window,
                    x: pos.0 * self.cell,
                    y: pos.1 * self.cell,
                },
            })
            .expect("surface id space exhausted");

        display.surface(id).unwrap().commit(self.ink(ink));

        // A synchronized sub-surface stays invisible until the parent is
        // committed again, so re-commit the window to bring this one on screen.
        display.surface(self.window).unwrap().commit(&self.bg);
        id
    }

    /// Any cell that is neither snake nor food. The board can fill up, in which
    /// case the food lands on the head and the game is effectively over anyway.
    fn free_cell(&mut self) -> Pos {
        for _ in 0..(GRID * GRID) * 2 {
            let pos = self.cell();
            let taken = self.body.iter().any(|t| t.pos == pos)
                || self.food.as_ref().is_some_and(|f| f.pos == pos);
            if !taken {
                return pos;
            }
        }
        self.body[0].pos
    }

    /// Put a tile where it belongs, with the right colour, and repaint.
    ///
    /// A synchronized sub-surface holds its state until the parent commits, so
    /// every tile change ends in a parent commit. It also has to: a commit that
    /// changes nothing visible can be dropped by the compositor, taking the
    /// tile's damage with it.
    fn place(&self, display: &mut Display, tile: &Tile, ink: Ink) {
        let surface = display.surface(tile.id).unwrap();
        surface.set_position(tile.pos.0 * self.cell, tile.pos.1 * self.cell);
        surface.commit(self.ink(ink));
        display.surface(self.window).unwrap().commit(&self.bg);
    }

    /// Lay out the opening position. Called once the real cell size is known.
    fn start(&mut self, display: &mut Display) {
        let head = Pos(GRID / 2, GRID / 2);
        self.dir = Pos(0, 1);
        self.wanted = self.dir;

        for i in 0..3 {
            let pos = head.step(Pos(0, -i));
            let ink = if i == 0 { Ink::Head } else { Ink::Body };
            let id = self.spawn(display, pos, ink);
            self.body.push(Tile { pos, id });
        }

        let pos = self.free_cell();
        let id = self.spawn(display, pos, Ink::Food);
        self.food = Some(Tile { pos, id });
    }

    fn step(&mut self, display: &mut Display) {
        if self.over {
            return;
        }

        // Doubling back into your own neck is not a legal move.
        if self.wanted != self.dir.opposite() {
            self.dir = self.wanted;
        }

        let next = self.body[0].pos.step(self.dir);
        if !inside(next) {
            self.over = true;
            println!("into the wall — score {}", self.score);
            return;
        }
        if self.body.iter().any(|t| t.pos == next) {
            self.over = true;
            // Leave a mark where it happened, so the board shows the mistake.
            let id = self.spawn(display, next, Ink::Food);
            self.body.push(Tile { pos: next, id });
            println!("into yourself — score {}", self.score);
            return;
        }

        // Whether this move lands on the food decides whether the tail survives,
        // so settle that before touching the body. Eating is what makes the
        // snake longer, and that works by *keeping* the tail this turn.
        let ate = self.food.as_ref().is_some_and(|f| f.pos == next);

        // The old head stays in the body and stays where it is — it is now a
        // middle segment. Only its colour changes, so its surface is reused.
        let head = self.body.first().expect("the snake always has a head");
        self.place(display, head, Ink::Body);

        if ate {
            // Growing, so the tail stays and a new tile is needed for the head.
            let id = self.spawn(display, next, Ink::Head);
            self.body.insert(0, Tile { pos: next, id });

            self.score += 1;
            println!("ate — score {}, length {}", self.score, self.body.len());

            // Pick the new cell before dropping the old food, so the old cell is
            // still excluded and the food cannot land where it already was.
            let pos = self.free_cell();
            let mut food = self.food.take().expect("checked just above");
            food.pos = pos;
            self.place(display, &food, Ink::Food);
            self.food = Some(food);
        } else {
            // The tail is not destroyed, it is simply moved to the front and
            // recoloured, so the number of surfaces stays at the snake's length
            // for the whole game. The head handled above stays in the body, so
            // one segment is taken off the end and one is put on — a wash.
            let mut tail = self.body.pop().expect("the snake always has a head");
            tail.pos = next;
            self.place(display, &tail, Ink::Head);
            self.body.insert(0, tail);
        }
    }

    fn turn(&mut self, dir: Pos) {
        self.wanted = dir;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut display = Display::new()?;

    let bg = display.add_color(Color::hex(0x18_1a_22));
    let inks = Inks {
        body: display.add_color(Color::hex(0x3c_d0_78)),
        head: display.add_color(Color::hex(0x9e_f0_c4)),
        food: display.add_color(Color::hex(0xe0_50_50)),
    };

    let window = display
        .add_surface(SurfaceInfo {
            width: GRID * CELL,
            height: GRID * CELL,
            role: SurfaceRole::Window {
                title: "snake".into(),
            },
        })
        .expect("surface id space exhausted");

    let mut game = Game::new(window, bg, inks);

    let mut socket = AsyncFd::new(Socket(display.connection_fd().as_raw_fd()))?;

    // The creation requests are still sitting in the write buffer. Push them out
    // before waiting on anything: the compositor cannot configure a surface it
    // has not been told about, so it would never answer, the socket would never
    // become readable, and the loop would hang on its very first await.
    display.flush()?;

    let mut stepper = tokio::time::interval(STEP);
    // A step that arrives late should not buy a burst of catch-up steps.
    stepper.set_missed_tick_behavior(MissedTickBehavior::Delay);

    println!(
        "w/a/s/d or h/j/k/l to steer, q to quit — one step every {}ms",
        STEP.as_millis()
    );

    loop {
        tokio::select! {
            // Biased, so a keypress is always handled before the step it might
            // have been meant to steer. Without it tokio picks a ready branch at
            // random, and a turn could be applied after the step that consumed
            // the tick it was meant for.
            biased;

            // The compositor has something to say.
            ready = socket.readable_mut() => {
                let mut ready = ready?;

                // Exactly one read per wakeup. `read` already drains the socket
                // until `WouldBlock`, and `prepare_read` only refuses when
                // *another* reader holds the claim — not when the buffer is
                // empty. Looping here to "read until nothing is left" therefore
                // spins forever: the claim is always granted and every read
                // after the first returns 0.
                if let Some(guard) = display.prepare_read() {
                    guard.read()?;
                }

                // Clear last, so nothing that arrived during the read is missed.
                ready.clear_ready();
            }

            // ...or it is time for the snake to move.
            _ = stepper.tick() => {
                // The first interval tick lands immediately, long before the
                // compositor's configure arrives, and until it does there is no
                // board and no snake to move. `cell` is the same "not laid out
                // yet" sentinel the configure branch tests.
                if game.cell != 0 && !game.over {
                    game.step(&mut display);
                }
            }
        }

        // Reading only pulls bytes off the socket. This is what runs the
        // handlers, and it is also what a skipped `prepare_read` asked for.
        display.dispatch_pending()?;
        display.flush()?;

        for event in display.events() {
            match event {
                Event::SurfaceEvent {
                    id,
                    event: SurfaceEvent::Configure { width, height },
                } => {
                    // Lay out once. Surfaces cannot be repositioned, so a later
                    // resize leaves the board where it was.
                    if id == window && game.cell == 0 {
                        game.cell = (width.min(height) / GRID).max(1);
                        // The window has now been configured, so this is the
                        // first commit that may legally carry a buffer.
                        display.surface(window).unwrap().commit(&game.bg);
                        game.start(&mut display);
                    }
                }
                Event::SeatEvent { id: seat, event } => match event {
                    SeatEvent::Key { pressed, key, .. } => {
                        if !pressed {
                            continue;
                        }
                        // The first keysym is the preferred one, and for a plain
                        // letter that is the character it types.
                        //
                        // Note: `char()` has no mapping for cursor keys, so the
                        // arrow keys cannot be detected this way — only letters
                        // work.
                        let Some(c) = display
                            .translate_key(seat, key)
                            .and_then(|keysyms| keysyms.first()?.char())
                        else {
                            continue;
                        };

                        match c.to_ascii_lowercase() {
                            'w' | 'k' => game.turn(Pos(0, -1)),
                            's' | 'j' => game.turn(Pos(0, 1)),
                            'a' | 'h' => game.turn(Pos(-1, 0)),
                            'd' | 'l' => game.turn(Pos(1, 0)),
                            'q' => return Ok(()),
                            _ => continue,
                        }
                    }
                    // Movement is on a timer now, so a repeat rate has nothing to
                    // do here.
                    SeatEvent::RepeatInfo { .. } => {}
                },
            }
        }

        if display.should_close(window) {
            break;
        }
    }

    println!("score {}", game.score);
    Ok(())
}
