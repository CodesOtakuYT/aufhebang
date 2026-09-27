use anyhow::Result;
use aufhebung::{
    display::Display,
    state::{Event, SeatEvent, SurfaceEvent},
    surface::{SurfaceInfo, SurfaceRole},
};

fn main() -> Result<()> {
    let mut display = Display::new()?;
    let red = display.add_color(u32::MAX, 0, 0, u32::MAX);
    let white = display.add_color(u32::MAX, u32::MAX, u32::MAX, u32::MAX);

    let window1 = display
        .add_surface(SurfaceInfo {
            width: 800,
            height: 800,
            role: SurfaceRole::Window {
                title: "Codotaku 1".into(),
            },
        })
        .expect("surface id space exhausted");
    let window2 = display
        .add_surface(SurfaceInfo {
            width: 800,
            height: 800,
            role: SurfaceRole::Window {
                title: "Codotaku 2".into(),
            },
        })
        .expect("surface id space exhausted");

    // Subsurfaces get no configure events, so they can be filled in right away.
    for x in 0..10 {
        for y in 0..10 {
            let id = display
                .add_surface(SurfaceInfo {
                    width: 64,
                    height: 64,
                    role: SurfaceRole::Subsurface {
                        parent: window2,
                        x: 10 + (64 + 1) * x,
                        y: 10 + (64 + 1) * y,
                    },
                })
                .expect("surface id space exhausted");
            display.surface(id).unwrap().commit(&red);
        }
    }

    let id = display
        .add_surface(SurfaceInfo {
            width: 10,
            height: 10,
            role: SurfaceRole::Subsurface {
                parent: window1,
                x: 10,
                y: 10,
            },
        })
        .expect("surface id space exhausted");
    display.surface(id).unwrap().commit(&red);

    loop {
        display.dispatch()?;
        for event in display.events() {
            match event {
                Event::SurfaceEvent { id, event } => match event {
                    SurfaceEvent::Configure { width, height } => {
                        println!("surface {id:?} configured to {width}x{height}");
                        // The library has already acked the configure, so the
                        // commit that answers it is ours to make.
                        display.surface(id).unwrap().commit(&white);
                    }
                },
                Event::SeatEvent { id, event } => match event {
                    SeatEvent::Key {
                        surface,
                        time,
                        key,
                        pressed,
                        ..
                    } => {
                        println!("key {key} on {surface:?} at {time} pressed={pressed}");
                        if let Some(keysyms) = display.translate_key(id, key) {
                            println!("  {keysyms:?}");
                        }
                    }
                    SeatEvent::RepeatInfo { rate, delay } => {
                        // This app implements no repeating, but a real one would
                        // start a timer here.
                        println!("key repeat: {rate}/s after {delay}ms");
                    }
                },
            }
        }

        let to_be_removed = display
            .surfaces()
            .filter(|&id| display.should_close(id))
            .collect::<Vec<_>>();
        for id in to_be_removed {
            display.remove_surface(id);
        }
        if display
            .surfaces()
            .filter(|&id| display.is_window(id))
            .count()
            == 0
        {
            break Ok(());
        }
    }
}
