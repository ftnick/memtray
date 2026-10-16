use std::{
    error::Error,
    time::{Duration, Instant},
};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::{GrayImage, Luma, Rgba, RgbaImage};
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use once_cell::sync::Lazy;
use sysinfo::System;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::WindowId,
};

const ICON_SIZE: u32 = 32;
const RENDER_SCALE: u32 = 6;
const BORDER_SIZE: u32 = 2;
const TEXT_PADDING: f32 = 2.0;
const TEXT_STROKE: i32 = 1;

const UPDATE_INTERVAL: Duration = Duration::from_secs(1);
const GIB: f64 = 1_073_741_824.0;

#[derive(Clone, Copy, Debug)]
struct MemorySnapshot<P = u32> {
    percent: P,
    used_gib: f64,
}

enum UserEvent {
    Menu(MenuEvent),
}

fn memory_stats(system: &mut System) -> MemorySnapshot<u32> {
    system.refresh_memory();

    let total = system.total_memory() as f64;
    let used = system.used_memory() as f64;

    if total <= 0.0 {
        return MemorySnapshot {
            percent: 0,
            used_gib: 0.0,
        };
    }

    let percent = ((used / total) * 100.0).round() as u32;

    MemorySnapshot {
        percent: percent.min(100),
        used_gib: used / GIB,
    }
}

fn icon_color(percent: u32) -> [u8; 4] {
    match percent {
        0..=59 => [40, 190, 100, 255],
        60..=79 => [240, 190, 40, 255],
        80..=89 => [245, 135, 40, 255],
        _ => [230, 65, 65, 255],
    }
}

fn render_icon<P>(percent: P) -> Result<Icon, Box<dyn Error>>
where
    P: Into<u32>,
{
    let percent = percent.into();
    let colour = icon_color(percent);
    let size = ICON_SIZE * RENDER_SCALE;
    let mut image = RgbaImage::from_pixel(size, size, Rgba([25, 25, 25, 255]));

    for x in 0..size {
        for y in 0..size {
            if x < BORDER_SIZE * RENDER_SCALE
                || y < BORDER_SIZE * RENDER_SCALE
                || x >= size - BORDER_SIZE * RENDER_SCALE
                || y >= size - BORDER_SIZE * RENDER_SCALE
            {
                image.put_pixel(x, y, Rgba(colour));
            }
        }
    }

    static FONT: Lazy<FontRef<'static>> = Lazy::new(|| {
        FontRef::try_from_slice(include_bytes!("../assets/RobotoCondensed-Bold.ttf"))
            .expect("font load")
    });

    let font = &*FONT;
    let text = percent.min(100).to_string();
    let base_size = 34.0 * RENDER_SCALE as f32;

    let initial_font = font.as_scaled(PxScale::from(base_size));
    let initial_width: f32 = text
        .chars()
        .map(|character| initial_font.h_advance(initial_font.glyph_id(character)))
        .sum();
    let initial_height = initial_font.height();

    let available = (ICON_SIZE as f32 - TEXT_PADDING * 2.0) * RENDER_SCALE as f32;
    let x_scale = base_size * (available / initial_width.max(1.0)).min(1.0);
    let y_scale = base_size * (available / initial_height.max(1.0)).min(1.0);

    let scaled_font = font.as_scaled(PxScale {
        x: x_scale,
        y: y_scale,
    });
    let text_width: f32 = text
        .chars()
        .map(|character| scaled_font.h_advance(scaled_font.glyph_id(character)))
        .sum();
    let text_height = scaled_font.height();
    let mut x = (size as f32 - text_width) / 2.0;
    let baseline = (size as f32 - text_height) / 2.0 + scaled_font.ascent();

    let mut text_mask = GrayImage::new(size, size);

    for character in text.chars() {
        let glyph = ab_glyph::Glyph {
            id: scaled_font.glyph_id(character),
            scale: scaled_font.scale(),
            position: ab_glyph::point(x, baseline),
        };

        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();

            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;

                if px < 0 || py < 0 || px >= size as i32 || py >= size as i32 {
                    return;
                }

                let alpha = (coverage * 255.0).round().clamp(0.0, 255.0) as u8;
                let pixel = text_mask.get_pixel_mut(px as u32, py as u32);

                if alpha > pixel[0] {
                    *pixel = Luma([alpha]);
                }
            });
        }

        x += scaled_font.h_advance(scaled_font.glyph_id(character));
    }

    let mut thickened_mask = GrayImage::new(size, size);

    for y in 0..size {
        for x in 0..size {
            let alpha = text_mask.get_pixel(x, y)[0];

            if alpha == 0 {
                continue;
            }

            for dy in -TEXT_STROKE..=TEXT_STROKE {
                for dx in -TEXT_STROKE..=TEXT_STROKE {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;

                    if nx < 0 || ny < 0 || nx >= size as i32 || ny >= size as i32 {
                        continue;
                    }

                    let pixel = thickened_mask.get_pixel_mut(nx as u32, ny as u32);

                    if alpha > pixel[0] {
                        *pixel = Luma([alpha]);
                    }
                }
            }
        }
    }

    for y in 0..size {
        for x in 0..size {
            let alpha = thickened_mask.get_pixel(x, y)[0];

            if alpha == 0 {
                continue;
            }

            let background = image.get_pixel(x, y);
            let amount = alpha as f32 / 255.0;
            let blend = |channel: u8| -> u8 {
                (channel as f32 * (1.0 - amount) + 255.0 * amount).round() as u8
            };

            image.put_pixel(
                x,
                y,
                Rgba([
                    blend(background[0]),
                    blend(background[1]),
                    blend(background[2]),
                    255,
                ]),
            );
        }
    }

    let image = image::imageops::resize(
        &image,
        ICON_SIZE,
        ICON_SIZE,
        image::imageops::FilterType::Lanczos3,
    );

    Icon::from_rgba(image.into_raw(), ICON_SIZE, ICON_SIZE).map_err(Into::into)
}

struct MemTrayApp {
    system: System,
    tray: Option<TrayIcon>,
    initial_icon: Icon,
    menu: Menu,
    exit_id: muda::MenuId,
    next_update: Instant,
    cached_percent: Option<u32>,
}

impl ApplicationHandler<UserEvent> for MemTrayApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.tray.is_some() {
            return;
        }

        match TrayIconBuilder::new()
            .with_tooltip("MemTray")
            .with_icon(self.initial_icon.clone())
            .with_menu(Box::new(self.menu.clone()))
            .with_menu_on_left_click(false)
            .build()
        {
            Ok(tray) => {
                self.tray = Some(tray);
                self.next_update = Instant::now() + UPDATE_INTERVAL;
                event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_update));
            }
            Err(error) => {
                eprintln!("Failed to create MemTray: {error}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        if let UserEvent::Menu(event) = event
            && event.id == self.exit_id
        {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if Instant::now() >= self.next_update {
            if let Some(tray) = self.tray.as_ref() {
                let snapshot = memory_stats(&mut self.system);

                if self.cached_percent != Some(snapshot.percent) {
                    match render_icon(snapshot.percent) {
                        Ok(icon) => {
                            let _ = tray.set_icon(Some(icon));
                            self.cached_percent = Some(snapshot.percent);
                        }
                        Err(error) => eprintln!("Failed to render MemTray icon: {error}"),
                    }
                }

                let _ = tray.set_tooltip(Some(format!(
                    "MemTray - RAM: {}% ({:.2} GiB used)",
                    snapshot.percent, snapshot.used_gib,
                )));
            }

            self.next_update = Instant::now() + UPDATE_INTERVAL;
        }

        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_update));
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

    MenuEvent::set_event_handler(Some(Box::new(move |event| {
        let _ = proxy.send_event(UserEvent::Menu(event));
    })));

    let menu = Menu::new();
    let version_item = MenuItem::new(
        format!("MemTray {}", env!("CARGO_PKG_VERSION")),
        false,
        None,
    );
    let exit_item = MenuItem::new("Exit", true, None);

    menu.append(&version_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&exit_item)?;

    let mut system = System::new();
    let initial_snapshot = memory_stats(&mut system);
    let initial_icon = render_icon(initial_snapshot.percent)?;

    let mut app = MemTrayApp {
        system,
        tray: None,
        initial_icon: initial_icon.clone(),
        menu,
        exit_id: exit_item.id().clone(),
        next_update: Instant::now(),
        cached_percent: Some(initial_snapshot.percent),
    };

    event_loop.run_app(&mut app)?;
    Ok(())
}
