use anyhow::Result;
use aufhebung::{
    display::Display,
    state::Event,
    surface::{SurfaceInfo, SurfaceRole},
};

fn main() -> Result<()> {
    let mut display = Display::new()?;
    let red = display.add_color(u32::MAX, 0, 0, u32::MAX);
    let white = display.add_color(u32::MAX, u32::MAX, u32::MAX, u32::MAX);

    let window1 = display.add_surface(SurfaceInfo {
        width: 800,
        height: 800,
        buffer: Some(white.clone()),
        role: SurfaceRole::Window {
            title: "Codotaku 1".into(),
        },
    })?;
    let window2 = display.add_surface(SurfaceInfo {
        width: 800,
        height: 800,
        buffer: Some(white.clone()),
        role: SurfaceRole::Window {
            title: "Codotaku 2".into(),
        },
    })?;
    for x in 0..10 {
        for y in 0..10 {
            display.add_surface(SurfaceInfo {
                width: 64,
                height: 64,
                buffer: Some(red.clone()),
                role: SurfaceRole::Subsurface {
                    parent: window2,
                    x: 10 + (64 + 1) * x,
                    y: 10 + (64 + 1) * y,
                },
            })?;
        }
    }
    display.add_surface(SurfaceInfo {
        width: 10,
        height: 10,
        buffer: Some(red),
        role: SurfaceRole::Subsurface {
            parent: window1,
            x: 10,
            y: 10,
        },
    })?;
    loop {
        display.dispatch()?;
        for event in display.events() {
            match event {
                Event::SurfaceEvent { id, event } => todo!(),
                Event::SeatEvent { id, event } => match event {
                    aufhebung::state::SeatEvent::Key {
                        surface,
                        time,
                        key,
                        group,
                        mods,
                    } => {
                        let keysyms = display.translate_key(id, key);
                        println!("{keysyms:?}");
                    }
                },
            }
        }
        let to_be_removed = display
            .surfaces()
            .filter(|&id| display.should_close(id))
            .collect::<Vec<_>>();
        if !to_be_removed.is_empty() {
            for id in to_be_removed {
                display.remove_surface(id);
            }
            if display
                .surfaces()
                .filter(|&s| display.is_window(s).unwrap())
                .count()
                == 0
            {
                break Ok(());
            }
        }
    }
}
