use std::{
    collections::VecDeque,
    error::Error,
    process::{Child, Command},
    time::{Duration, Instant},
};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use eframe::egui;
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
const HISTORY_INTERVAL: Duration = Duration::from_secs(1);
const HISTORY_LENGTH: usize = 150;
const GIB: f64 = 1_073_741_824.0;

enum UserEvent {
    Menu(MenuEvent),
}

fn memory_stats(system: &mut System) -> (u32, f64, f64) {
    system.refresh_memory();

    let total = system.total_memory() as f64;
    let used = system.used_memory() as f64;

    if total <= 0.0 {
        return (0, 0.0, 0.0);
    }

    let percent = (used / total * 100.0).round() as u32;
    let available = (total - used).max(0.0);

    (percent.min(100), used / GIB, available / GIB)
}

fn render_icon(percent: u32) -> Result<Icon, Box<dyn Error>> {
    let colour = match percent {
        0..=59 => [40, 190, 100, 255],
        60..=79 => [240, 190, 40, 255],
        80..=89 => [245, 135, 40, 255],
        _ => [230, 65, 65, 255],
    };

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

fn launch_history(child: &mut Option<Child>) {
    if let Some(process) = child.as_mut() {
        match process.try_wait() {
            Ok(None) => return,
            Ok(Some(_)) | Err(_) => *child = None,
        }
    }

    let result = std::env::current_exe()
        .and_then(|executable| Command::new(executable).arg("--history").spawn());

    match result {
        Ok(process) => *child = Some(process),
        Err(error) => {
            eprintln!("Faled to open RAM history: {error}");
        }
    }
}

fn close_history(child: &mut Option<Child>) {
    if let Some(mut process) = child.take()
        && matches!(process.try_wait(), Ok(None))
    {
        let _ = process.kill();
        let _ = process.wait();
    }
}

struct MemTrayApp {
    system: System,
    tray: Option<TrayIcon>,
    initial_icon: Icon,
    menu: Menu,

    history_id: muda::MenuId,
    exit_id: muda::MenuId,

    history_child: Option<Child>,
    next_update: Instant,
    cached_percent: Option<u32>,
    cached_icon: Option<Icon>,
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
        // nno window
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Menu(event) if event.id == self.history_id => {
                launch_history(&mut self.history_child);
            }
            UserEvent::Menu(event) if event.id == self.exit_id => {
                close_history(&mut self.history_child);
                event_loop.exit();
            }
            UserEvent::Menu(_) => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if Instant::now() >= self.next_update {
            if let Some(tray) = self.tray.as_ref() {
                let (percent, used, _) = memory_stats(&mut self.system);

                // ?
                if self.cached_percent != Some(percent) {
                    match render_icon(percent) {
                        Ok(icon) => {
                            let _ = tray.set_icon(Some(icon.clone()));
                            self.cached_icon = Some(icon);
                            self.cached_percent = Some(percent);
                        }
                        Err(error) => eprintln!("Failed to render MemTray icon: {error}"),
                    }
                }

                let _ = tray.set_tooltip(Some(format!(
                    "MemTray - RAM: {}% ({:.2} GiB used)",
                    self.cached_percent.unwrap_or(percent),
                    used,
                )));
            }

            self.next_update = Instant::now() + UPDATE_INTERVAL;
        }

        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_update));
    }
}

struct HistoryApp {
    system: System,
    samples: VecDeque<f32>,
    next_sample: Instant,
    latest_percent: f32,
    latest_used: f64,
    latest_available: f64,
}

impl HistoryApp {
    fn new() -> Self {
        let mut system = System::new();
        let (percent, used, available) = memory_stats(&mut system);

        let mut samples = VecDeque::with_capacity(HISTORY_LENGTH);
        samples.push_back(percent as f32);

        Self {
            system,
            samples,
            next_sample: Instant::now() + HISTORY_INTERVAL,
            latest_percent: percent as f32,
            latest_used: used,
            latest_available: available,
        }
    }

    fn sample_memory(&mut self) {
        let (percent, used, available) = memory_stats(&mut self.system);

        self.latest_percent = percent as f32;
        self.latest_used = used;
        self.latest_available = available;

        self.samples.push_back(self.latest_percent);

        if self.samples.len() > HISTORY_LENGTH {
            self.samples.pop_front();
        }

        self.next_sample = Instant::now() + HISTORY_INTERVAL;
    }
}

impl eframe::App for HistoryApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if Instant::now() >= self.next_sample {
            self.sample_memory();
        }

        ui.ctx().request_repaint_after(Duration::from_millis(250));

        ui.heading("RAM usage history");

        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(format!("{:.0}% used", self.latest_percent))
                    .size(21.0)
                    .strong(),
            );

            ui.separator();

            ui.label(format!("{:.2} GiB used", self.latest_used));
            ui.label(format!("{:.2} GiB available", self.latest_available));
        });

        ui.add_space(12.0);

        let chart_height = ui.available_height().max(190.0);
        let chart_size = egui::vec2(ui.available_width(), chart_height);

        let (response, painter) = ui.allocate_painter(chart_size, egui::Sense::hover());

        let outer = response.rect;

        painter.rect_filled(outer, 8.0, egui::Color32::from_rgb(25, 27, 31));

        let chart = egui::Rect::from_min_max(
            egui::pos2(outer.left() + 43.0, outer.top() + 15.0),
            egui::pos2(outer.right() - 13.0, outer.bottom() - 29.0),
        );

        for value in [0, 25, 50, 75, 100] {
            let fraction = value as f32 / 100.0;
            let y = chart.bottom() - fraction * chart.height();

            painter.line_segment(
                [egui::pos2(chart.left(), y), egui::pos2(chart.right(), y)],
                egui::Stroke::new(1.0, egui::Color32::from_rgb(55, 59, 66)),
            );

            painter.text(
                egui::pos2(outer.left() + 34.0, y),
                egui::Align2::RIGHT_CENTER,
                format!("{value}%"),
                egui::FontId::proportional(11.0),
                egui::Color32::from_rgb(170, 175, 184),
            );
        }

        if !self.samples.is_empty() {
            let count = self.samples.len();

            let points: Vec<egui::Pos2> = self
                .samples
                .iter()
                .enumerate()
                .map(|(index, percent)| {
                    let x_fraction = if count <= 1 {
                        1.0
                    } else {
                        index as f32 / (count - 1) as f32
                    };

                    let y_fraction = percent.clamp(0.0, 100.0) / 100.0;

                    egui::pos2(
                        chart.left() + x_fraction * chart.width(),
                        chart.bottom() - y_fraction * chart.height(),
                    )
                })
                .collect();

            for pair in points.windows(2) {
                painter.line_segment(
                    [pair[0], pair[1]],
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(70, 190, 235)),
                );
            }

            if let Some(last) = points.last() {
                painter.circle_filled(*last, 4.0, egui::Color32::from_rgb(110, 220, 255));
            }
        }

        painter.text(
            egui::pos2(chart.left(), outer.bottom() - 14.0),
            egui::Align2::LEFT_CENTER,
            "5 minutes ago",
            egui::FontId::proportional(10.0),
            egui::Color32::from_rgb(170, 175, 184),
        );

        painter.text(
            egui::pos2(chart.right(), outer.bottom() - 14.0),
            egui::Align2::RIGHT_CENTER,
            "Now",
            egui::FontId::proportional(10.0),
            egui::Color32::from_rgb(170, 175, 184),
        );
    }
}

fn run_history_window() -> Result<(), Box<dyn Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([680.0, 420.0])
            .with_min_inner_size([500.0, 320.0]),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "MemTray - RAM History",
        options,
        Box::new(|context| {
            context.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(HistoryApp::new()))
        }),
    )?;

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().nth(1).as_deref() == Some("--history") {
        return run_history_window();
    }

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

    let history_item = MenuItem::new("Usage history", true, None);
    let exit_item = MenuItem::new("Exit", true, None);

    menu.append(&version_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&history_item)?;
    menu.append(&exit_item)?;

    let mut system = System::new();
    let (initial_percent, _, _) = memory_stats(&mut system);

    let initial_icon = render_icon(initial_percent)?;

    let mut app = MemTrayApp {
        system,
        tray: None,
        initial_icon: initial_icon.clone(),
        menu,
        history_id: history_item.id().clone(),
        exit_id: exit_item.id().clone(),
        history_child: None,
        next_update: Instant::now(),
        cached_percent: Some(initial_percent),
        cached_icon: Some(initial_icon),
    };

    event_loop.run_app(&mut app)?;

    Ok(())
}
