use eframe::egui::{self, Color32, RichText, Rounding, Stroke, Vec2};

pub const BG_MAIN: Color32 = Color32::from_rgb(18, 21, 30);
pub const BG_SIDEBAR: Color32 = Color32::from_rgb(13, 15, 22);
pub const BG_CARD: Color32 = Color32::from_rgb(26, 30, 44);
pub const ACCENT_ORANGE: Color32 = Color32::from_rgb(249, 115, 22);
pub const ACCENT_GREEN: Color32 = Color32::from_rgb(16, 185, 129);
pub const ACCENT_PURPLE: Color32 = Color32::from_rgb(139, 92, 246);
pub const ACCENT_CYAN: Color32 = Color32::from_rgb(6, 182, 212);
pub const ACCENT_RED: Color32 = Color32::from_rgb(239, 68, 68);

pub fn setup_modern_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG_MAIN;
    visuals.window_fill = BG_CARD;
    visuals.window_rounding = Rounding::same(12.0);
    visuals.window_stroke = Stroke::new(1.5, Color32::from_rgb(55, 65, 95));
    visuals.selection.bg_fill = ACCENT_ORANGE;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);

    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(20, 24, 35);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(35, 41, 58);
    visuals.widgets.inactive.rounding = Rounding::same(6.0);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(50, 58, 82);
    visuals.widgets.hovered.rounding = Rounding::same(6.0);
    visuals.widgets.active.bg_fill = ACCENT_ORANGE;
    visuals.widgets.active.rounding = Rounding::same(6.0);

    ctx.set_visuals(visuals);
}

pub fn file_avatar_style(file_name: &str) -> (&'static str, &'static str, Color32) {
    let lower = file_name.to_lowercase();
    if lower.ends_with(".iso") || lower.ends_with(".dmg") {
        ("💿", "ISO", Color32::from_rgb(234, 88, 12))
    } else if lower.ends_with(".torrent") || lower.starts_with("magnet:") {
        ("⚡", "TOR", Color32::from_rgb(16, 185, 129))
    } else if lower.ends_with(".zip") || lower.ends_with(".rar") || lower.ends_with(".7z") {
        ("♦", "ZIP", Color32::from_rgb(217, 119, 6))
    } else if lower.ends_with(".exe") || lower.ends_with(".msi") {
        ("▶", "EXE", Color32::from_rgb(59, 130, 246))
    } else if lower.ends_with(".mp4") || lower.ends_with(".mkv") {
        ("🎬", "VID", Color32::from_rgb(168, 85, 247))
    } else {
        ("📄", "FILE", Color32::from_rgb(100, 116, 139))
    }
}

pub fn draw_file_avatar(ui: &mut egui::Ui, file_name: &str, size: f32) {
    let (icon_sym, ext_label, avatar_color) = file_avatar_style(file_name);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size, size), egui::Sense::hover());
    ui.painter().rect_filled(rect, Rounding::same(size * 0.22), avatar_color);
    ui.painter().rect_stroke(
        rect,
        Rounding::same(size * 0.22),
        Stroke::new(1.5, Color32::from_white_alpha(45)),
    );
    ui.painter().text(
        rect.center() - Vec2::new(0.0, size * 0.15),
        egui::Align2::CENTER_CENTER,
        icon_sym,
        egui::FontId::proportional(size * 0.38),
        Color32::WHITE,
    );
    ui.painter().text(
        rect.center() + Vec2::new(0.0, size * 0.23),
        egui::Align2::CENTER_CENTER,
        ext_label,
        egui::FontId::proportional(size * 0.20),
        Color32::WHITE,
    );
}

pub fn draw_modern_bar(ui: &mut egui::Ui, progress: f32, fill_color: Color32, height: f32, text: Option<String>) {
    let desired_size = Vec2::new(ui.available_width(), height);
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let rounding = Rounding::same(height * 0.45);

    ui.painter().rect_filled(rect, rounding, Color32::from_rgb(14, 17, 25));
    ui.painter().rect_stroke(rect, rounding, Stroke::new(1.0, Color32::from_rgb(38, 44, 64)));

    let clamped = progress.clamp(0.0, 1.0);
    if clamped > 0.0 {
        let mut fill_rect = rect;
        fill_rect.set_width((rect.width() * clamped).max(height));
        ui.painter().rect_filled(fill_rect, rounding, fill_color);
    }

    if let Some(txt) = text {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            txt,
            egui::FontId::proportional(11.5),
            Color32::WHITE,
        );
    }
}

pub fn draw_badge(ui: &mut egui::Ui, text: &str, bg_color: Color32, text_color: Color32) {
    egui::Frame::none()
        .fill(bg_color)
        .rounding(Rounding::same(5.0))
        .inner_margin(egui::Margin::symmetric(7.0, 2.5))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(text_color).size(11.5).strong());
        });
}