use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::CStr;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;

use directories::ProjectDirs;
use fltk::{
    app,
    button::{Button, CheckButton},
    dialog::{FileDialogType, NativeFileChooser},
    draw,
    enums::{Align, Color, Event, Font, FrameType, Key, Shortcut},
    frame::Frame,
    group::{Flex, Group},
    image::PngImage,
    menu::{MacAppMenu, MenuFlag, MenuItem, SysMenuBar, WindowMenuStyle},
    misc::Spinner,
    prelude::*,
    text::{TextBuffer, TextDisplay, WrapMode},
    window::Window,
};
use serde::{Deserialize, Serialize};

const NOTICE_TICK_SECONDS: f64 = 0.1;
const FOCUS_HIDE_DELAY_SECONDS: f64 = 0.075;
const DEFAULT_OPACITY: f64 = 1.0;
const DEFAULT_WINDOW_W: i32 = 500;
const DEFAULT_WINDOW_H: i32 = 500;
const LEGACY_DEFAULT_WINDOW_H: i32 = 800;
const LOGO_PNG: &[u8] = include_bytes!("../logo.png");
const BLURRY_PNG: &[u8] = include_bytes!("../blurry.png");

static OPEN_FILE_SENDER: OnceLock<app::Sender<Msg>> = OnceLock::new();

macro_rules! debug_log {
    ($($arg:tt)*) => {
        eprintln!("[blurred] {}", format!($($arg)*));
    };
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct SavedState {
    always_visible: bool,
    auto_show: bool,
    hide_on_copy: bool,
    dark_mode: bool,
    opacity: f64,
    last_file: Option<PathBuf>,
    window_x: i32,
    window_y: i32,
    window_w: i32,
    window_h: i32,
}

impl Default for SavedState {
    fn default() -> Self {
        Self {
            always_visible: false,
            auto_show: true,
            hide_on_copy: true,
            dark_mode: false,
            opacity: DEFAULT_OPACITY,
            last_file: None,
            window_x: 100,
            window_y: 100,
            window_w: DEFAULT_WINDOW_W,
            window_h: DEFAULT_WINDOW_H,
        }
    }
}

struct AppState {
    current_file: Option<PathBuf>,
    visibility: VisibilityState,
    always_visible: bool,
    auto_show: bool,
    hide_on_copy: bool,
    dark_mode: bool,
    opacity: f64,
    copied_notice_until: Option<Instant>,
    last_error: Option<String>,
    settings_dirty: bool,
    settings: SavedState,
    focus_generation: u64,
}

impl AppState {
    fn load() -> Self {
        let settings = load_saved_state();
        Self {
            current_file: settings.last_file.clone(),
            visibility: VisibilityState::Visible,
            always_visible: settings.always_visible,
            auto_show: settings.auto_show,
            hide_on_copy: settings.hide_on_copy,
            dark_mode: settings.dark_mode,
            opacity: settings.opacity,
            copied_notice_until: None,
            last_error: None,
            settings_dirty: false,
            settings,
            focus_generation: 0,
        }
    }

    fn apply(&mut self, action: StateAction) -> StateEffects {
        match action {
            StateAction::FileLoaded { path } => {
                self.current_file = Some(path.clone());
                self.settings.last_file = Some(path);
                self.visibility = VisibilityState::Visible;
                self.copied_notice_until = None;
                self.last_error = None;
                self.settings_dirty = true;
                StateEffects {
                    sync_title: true,
                    apply_visibility: true,
                    focus_editor: true,
                    ..StateEffects::default()
                }
            }
            StateAction::FileLoadFailed(err) => {
                self.last_error = Some(err);
                StateEffects {
                    redraw_hidden_preview: true,
                    ..StateEffects::default()
                }
            }
            StateAction::Show => {
                let changed = self.show();
                StateEffects {
                    apply_visibility: changed,
                    focus_editor: changed,
                    ..StateEffects::default()
                }
            }
            StateAction::HideTemporarily => {
                let changed = self.hide(HideReason::Temporary {
                    ignored_focus_after_hide: false,
                });
                StateEffects {
                    apply_visibility: changed,
                    redraw_hidden_preview: changed,
                    ..StateEffects::default()
                }
            }
            StateAction::SetAlwaysVisible(enabled) => {
                self.always_visible = enabled;
                self.settings_dirty = true;
                let mut effects = StateEffects {
                    sync_menu: true,
                    ..StateEffects::default()
                };
                if enabled {
                    let changed = self.show();
                    effects.apply_visibility = changed;
                    effects.focus_editor = changed;
                }
                effects
            }
            StateAction::SetAutoShow(enabled) => {
                self.auto_show = enabled;
                self.settings_dirty = true;
                StateEffects {
                    sync_menu: true,
                    ..StateEffects::default()
                }
            }
            StateAction::Focused => {
                self.focus_generation = self.focus_generation.wrapping_add(1);
                if matches!(
                    self.visibility,
                    VisibilityState::Hidden(HideReason::Temporary {
                        ignored_focus_after_hide: false
                    })
                ) {
                    self.visibility = VisibilityState::Hidden(HideReason::Temporary {
                        ignored_focus_after_hide: true,
                    });
                    StateEffects::default()
                } else if self.is_visible() || !self.should_auto_show_on_focus() {
                    StateEffects::default()
                } else {
                    let changed = self.show();
                    StateEffects {
                        apply_visibility: changed,
                        focus_editor: changed,
                        ..StateEffects::default()
                    }
                }
            }
            StateAction::Unfocused => {
                if self.always_visible || !self.is_visible() {
                    if matches!(
                        self.visibility,
                        VisibilityState::Hidden(HideReason::Temporary {
                            ignored_focus_after_hide: true
                        })
                    ) {
                        self.visibility = VisibilityState::Hidden(HideReason::FocusLoss);
                    }
                    StateEffects::default()
                } else {
                    self.focus_generation = self.focus_generation.wrapping_add(1);
                    StateEffects {
                        schedule_hide_generation: Some(self.focus_generation),
                        ..StateEffects::default()
                    }
                }
            }
            StateAction::DeferredHideOnUnfocus(expected_generation) => {
                if self.focus_generation == expected_generation && !self.always_visible {
                    let changed = self.hide(HideReason::FocusLoss);
                    StateEffects {
                        apply_visibility: changed,
                        redraw_hidden_preview: changed,
                        ..StateEffects::default()
                    }
                } else {
                    StateEffects::default()
                }
            }
            StateAction::Copied => {
                let should_hide = self.hide_on_copy && !self.always_visible && self.is_visible();
                debug_log!("copied should_hide={should_hide}");
                if should_hide {
                    let changed = self.hide(HideReason::CopyNotice);
                    StateEffects {
                        apply_visibility: changed,
                        redraw_hidden_preview: true,
                        ..StateEffects::default()
                    }
                } else {
                    StateEffects::default()
                }
            }
            StateAction::Tick => {
                if self
                    .copied_notice_until
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    self.copied_notice_until = None;
                    StateEffects {
                        redraw_hidden_preview: true,
                        ..StateEffects::default()
                    }
                } else {
                    StateEffects::default()
                }
            }
        }
    }

    fn should_auto_show_on_focus(&self) -> bool {
        if matches!(
            self.visibility,
            VisibilityState::Hidden(HideReason::Temporary { .. })
        ) {
            return false;
        }

        if self
            .copied_notice_until
            .is_some_and(|deadline| Instant::now() < deadline)
        {
            return false;
        }

        self.auto_show || self.always_visible
    }

    fn is_visible(&self) -> bool {
        matches!(self.visibility, VisibilityState::Visible)
    }

    fn show(&mut self) -> bool {
        let changed = !self.is_visible() || self.copied_notice_until.is_some();
        self.visibility = VisibilityState::Visible;
        self.copied_notice_until = None;
        changed
    }

    fn hide(&mut self, reason: HideReason) -> bool {
        if self.always_visible {
            return false;
        }

        let changed = self.is_visible();
        self.visibility = VisibilityState::Hidden(reason);
        self.copied_notice_until = match reason {
            HideReason::CopyNotice => Some(Instant::now() + std::time::Duration::from_secs(1)),
            HideReason::Temporary { .. } | HideReason::FocusLoss => None,
        };
        changed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VisibilityState {
    Visible,
    Hidden(HideReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HideReason {
    Temporary { ignored_focus_after_hide: bool },
    FocusLoss,
    CopyNotice,
}

#[derive(Default)]
struct StateEffects {
    sync_title: bool,
    sync_menu: bool,
    apply_visibility: bool,
    focus_editor: bool,
    redraw_hidden_preview: bool,
    schedule_hide_generation: Option<u64>,
}

enum StateAction {
    FileLoaded { path: PathBuf },
    FileLoadFailed(String),
    Show,
    HideTemporarily,
    SetAlwaysVisible(bool),
    SetAutoShow(bool),
    Focused,
    Unfocused,
    DeferredHideOnUnfocus(u64),
    Copied,
    Tick,
}

#[derive(Clone)]
enum Msg {
    Open,
    Reload,
    OpenPath(PathBuf),
    OpenSettings,
    Show,
    Hide,
    Quit,
    ToggleAlwaysVisible,
    ToggleAutoShow,
    Copied,
    Tick,
    Focused,
    Unfocused,
    DeferredHideOnUnfocus(u64),
    Resized,
}

fn main() {
    let app = app::App::default().with_scheme(app::Scheme::Gtk);
    app::set_scrollbar_size(10);
    let (sender, receiver) = app::channel::<Msg>();
    let _ = OPEN_FILE_SENDER.set(sender);

    let state = Rc::new(RefCell::new(AppState::load()));

    let initial = state.borrow().settings.clone();
    let (initial_x, initial_y, initial_w, initial_h) = initial_window_rect(&initial);
    let mut wind = Window::new(
        initial_x,
        initial_y,
        initial_w,
        initial_h,
        "No file currently open",
    );
    wind.make_resizable(true);
    wind.set_color(Color::White);
    wind.set_opacity(initial.opacity);
    if let Some(icon) = load_logo_image() {
        wind.set_icon(Some(icon));
    }

    let mut root = Flex::default_fill().column();
    root.set_margin(0);
    root.set_pad(0);

    #[cfg(target_os = "macos")]
    {
        SysMenuBar::set_window_menu_style(WindowMenuStyle::TabbingModeNone);
        MacAppMenu::set_print("");
        MacAppMenu::set_print_no_titlebar("");
        MacAppMenu::set_toggle_print_titlebar("");
    }

    let mut menu = SysMenuBar::default().with_size(0, 30);
    menu.set_frame(FrameType::FlatBox);
    menu.add_emit(
        "&File/Open...\t",
        Shortcut::Ctrl | 'o',
        MenuFlag::Normal,
        sender,
        Msg::Open,
    );
    menu.add_emit(
        "&File/Reload\t",
        Shortcut::Ctrl | 'r',
        MenuFlag::Normal,
        sender,
        Msg::Reload,
    );
    menu.add_emit(
        "&Options/Always Visible\t",
        Shortcut::None,
        MenuFlag::Toggle,
        sender,
        Msg::ToggleAlwaysVisible,
    );
    menu.add_emit(
        "&Options/Auto Show\t",
        Shortcut::None,
        MenuFlag::Toggle,
        sender,
        Msg::ToggleAutoShow,
    );
    menu.add_emit(
        "&Options/Settings...\t",
        Shortcut::None,
        MenuFlag::Normal,
        sender,
        Msg::OpenSettings,
    );
    menu.add_emit(
        "&View/Show\t",
        Shortcut::None,
        MenuFlag::Normal,
        sender,
        Msg::Show,
    );
    menu.add_emit(
        "&View/Hide\t",
        Shortcut::None,
        MenuFlag::Normal,
        sender,
        Msg::Hide,
    );
    menu.add_emit(
        "&File/Quit\t",
        Shortcut::Ctrl | 'q',
        MenuFlag::Normal,
        sender,
        Msg::Quit,
    );
    sync_menu_state(&state, &mut menu);
    root.fixed(&menu, 30);

    let mut visible_group = Group::default_fill();
    let mut visible_flex = Flex::default_fill().column();
    visible_flex.set_margin(0);
    visible_flex.set_pad(0);

    let mut editor = TextDisplay::default_fill();
    editor.set_linenumber_width(0);
    editor.set_scrollbar_size(10);
    editor.set_text_font(Font::Courier);
    editor.set_text_size(16);
    editor.set_frame(FrameType::FlatBox);
    editor.set_color(Color::White);
    editor.wrap_mode(WrapMode::AtBounds, 0);
    let mut text_buffer = TextBuffer::default();
    editor.set_buffer(Some(text_buffer.clone()));
    visible_flex.end();
    visible_group.end();

    let mut hidden_group = Group::default_fill();
    let mut hidden_flex = Flex::default_fill().column();
    hidden_flex.set_margin(0);
    hidden_flex.set_pad(0);
    let mut hidden_preview = Frame::default_fill();
    hidden_preview.set_frame(FrameType::FlatBox);
    hidden_preview.set_color(Color::from_rgb(244, 247, 249));
    let mut show_button_row = Flex::default().row();
    show_button_row.set_margin(12);
    show_button_row.set_pad(0);
    let left_spacer = Frame::default();
    let mut show_btn = Button::default().with_label("Show");
    show_btn.clear_visible_focus();
    let right_spacer = Frame::default();
    show_button_row.fixed(&left_spacer, 0);
    show_button_row.fixed(&show_btn, 100);
    show_button_row.fixed(&right_spacer, 0);
    show_button_row.end();
    hidden_flex.fixed(&show_button_row, 56);
    hidden_flex.end();
    hidden_group.end();

    root.end();
    wind.end();
    wind.show_with_env_args();
    wind.set_opacity(initial.opacity);

    #[cfg(target_os = "macos")]
    app::raw_open_callback(Some(handle_open_file));

    show_btn.emit(sender, Msg::Show);

    let blurred_image = load_blurred_image();
    let blurred_image = Rc::new(RefCell::new(blurred_image));

    let state_for_draw = state.clone();
    let image_for_draw = blurred_image.clone();
    hidden_preview.draw(move |f| {
        let state = state_for_draw.borrow();
        draw::set_draw_color(Color::from_rgb(244, 247, 249));
        draw::draw_rectf(f.x(), f.y(), f.w(), f.h());

        if let Some(image) = image_for_draw.borrow().as_ref() {
            let mut scaled = image.clone();
            let image_width = image.data_w().max(1) as f32;
            let image_height = image.data_h().max(1) as f32;
            let width_scale = f.w().max(1) as f32 / image_width;
            let height_scale = f.h().max(1) as f32 / image_height;
            let scale = width_scale.max(height_scale);
            let scaled_width = (image_width * scale).round().max(1.0) as i32;
            let scaled_height = (image_height * scale).round().max(1.0) as i32;
            let draw_x = f.x() + ((f.w() - scaled_width) / 2);
            let draw_y = f.y() + ((f.h() - scaled_height) / 2);
            scaled.scale(scaled_width, scaled_height, false, true);
            scaled.draw(draw_x, draw_y, scaled_width, scaled_height);
        }

        if let Some(error) = &state.last_error {
            draw::set_font(Font::HelveticaItalic, 14);
            draw::set_draw_color(Color::from_rgb(160, 40, 40));
            draw::draw_text2(
                error,
                f.x() + 16,
                f.y() + 8,
                f.w() - 32,
                24,
                Align::Left | Align::Inside,
            );
        }

        if state
            .copied_notice_until
            .is_some_and(|deadline| Instant::now() < deadline)
        {
            let notice_w = 220;
            let notice_h = 34;
            let notice_x = f.x() + ((f.w() - notice_w) / 2);
            let notice_y = f.y() + ((f.h() - notice_h) / 2);
            draw::set_draw_color(Color::from_rgb(32, 35, 39));
            draw::draw_rounded_rectf(notice_x, notice_y, notice_w, notice_h, 8);
            draw::set_font(Font::HelveticaBold, 14);
            draw::set_draw_color(Color::White);
            draw::draw_text2(
                "Copied to clipboard",
                notice_x,
                notice_y,
                notice_w,
                notice_h,
                Align::Center | Align::Inside,
            );
        }
    });

    let mut settings_win = Window::new(220, 180, 380, 250, "Settings");
    settings_win.make_modal(true);
    let mut settings_flex = Flex::default_fill().column();
    settings_flex.set_margin(12);
    settings_flex.set_pad(10);

    let mut settings_hide_on_copy = CheckButton::default().with_label("Hide On Copy");
    settings_hide_on_copy.set_value(state.borrow().hide_on_copy);

    let mut settings_dark = CheckButton::default().with_label("Dark Mode");
    settings_dark.set_value(state.borrow().dark_mode);
    let mut opacity_row = Flex::default().row();
    opacity_row.set_pad(8);
    let mut opacity_label = Frame::default().with_label("Window opacity (%)");
    opacity_label.set_align(Align::Left | Align::Inside);
    let mut opacity_spinner = Spinner::default();
    opacity_spinner.set_range(35.0, 100.0);
    opacity_spinner.set_step(5.0);
    opacity_spinner.set_value((state.borrow().opacity * 100.0).round());
    opacity_row.fixed(&opacity_label, 140);
    opacity_row.fixed(&opacity_spinner, 120);
    opacity_row.end();
    let mut settings_close = Button::default().with_label("Close");

    settings_flex.fixed(&settings_hide_on_copy, 28);
    settings_flex.fixed(&settings_dark, 28);
    settings_flex.fixed(&opacity_row, 32);
    settings_flex.fixed(&settings_close, 34);
    settings_flex.end();
    settings_win.end();
    settings_win.hide();

    let settings_state = state.clone();
    settings_hide_on_copy.set_callback(move |check| {
        let mut s = settings_state.borrow_mut();
        s.hide_on_copy = check.value();
        s.settings_dirty = true;
        drop(s);
        persist_current_settings(&settings_state);
    });

    let settings_state = state.clone();
    let mut wind_for_opacity = wind.clone();
    opacity_spinner.set_callback(move |spinner| {
        let opacity = (spinner.value() / 100.0).clamp(0.35, 1.0);
        let mut s = settings_state.borrow_mut();
        s.opacity = opacity;
        s.settings_dirty = true;
        drop(s);
        wind_for_opacity.set_opacity(opacity);
        persist_current_settings(&settings_state);
    });

    let mut settings_win_close = settings_win.clone();
    settings_close.set_callback(move |_| {
        settings_win_close.hide();
    });

    let settings_state = state.clone();
    let mut wind_for_dark = wind.clone();
    let mut hidden_for_dark = hidden_preview.clone();
    let mut editor_for_dark = editor.clone();
    let mut settings_for_dark = settings_win.clone();
    let mut settings_hide_on_copy_for_dark = settings_hide_on_copy.clone();
    let mut settings_dark_for_dark = settings_dark.clone();
    let mut opacity_label_for_dark = opacity_label.clone();
    let mut opacity_spinner_for_dark = opacity_spinner.clone();
    let mut settings_close_for_dark = settings_close.clone();
    settings_dark.set_callback(move |check| {
        let mut s = settings_state.borrow_mut();
        s.dark_mode = check.value();
        s.settings_dirty = true;
        let dark_mode = s.dark_mode;
        drop(s);
        apply_theme(
            dark_mode,
            &mut wind_for_dark,
            &mut editor_for_dark,
            &mut hidden_for_dark,
            &mut settings_for_dark,
            &mut settings_hide_on_copy_for_dark,
            &mut settings_dark_for_dark,
            &mut opacity_label_for_dark,
            &mut opacity_spinner_for_dark,
            &mut settings_close_for_dark,
        );
        persist_current_settings(&settings_state);
    });

    #[cfg(target_os = "macos")]
    {
        MacAppMenu::set_about("About blurred");

        let app_menu_items = MenuItem::new(&["Buy me a coffee"]);
        if let Some(mut item) = app_menu_items.at(0) {
            item.set_callback(|_| {
                let _ = open_external_url("https://www.buymeacoffee.com/byteface");
            });
        }
        MacAppMenu::custom_application_menu_items(app_menu_items.clone());
        let _app_menu_items = app_menu_items;
    }

    let editor_buffer = text_buffer.clone();
    let editor_sender = sender;
    editor.handle(move |_ed, ev| match ev {
        Event::KeyDown | Event::Shortcut => {
            if is_copy_event() && !editor_buffer.selection_text().is_empty() {
                debug_log!("copy shortcut detected");
                editor_sender.send(Msg::Copied);
            }
            false
        }
        _ => false,
    });

    let sender_for_focus = sender;
    wind.handle(move |_w, ev| match ev {
        Event::Focus => {
            sender_for_focus.send(Msg::Focused);
            false
        }
        Event::Unfocus => {
            sender_for_focus.send(Msg::Unfocused);
            false
        }
        Event::Resize => {
            sender_for_focus.send(Msg::Resized);
            false
        }
        _ => false,
    });

    let state_for_tick = state.clone();
    let sender_for_tick = sender;
    app::add_timeout3(NOTICE_TICK_SECONDS, move |handle| {
        let should_tick = state_for_tick.borrow().copied_notice_until.is_some();
        if should_tick {
            sender_for_tick.send(Msg::Tick);
        }
        app::repeat_timeout3(NOTICE_TICK_SECONDS, handle);
    });

    let startup_file = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .find(|path| path.exists() && path.is_file());
    let load_last = startup_file.or_else(|| state.borrow().settings.last_file.clone());
    if let Some(path) = load_last {
        load_file_into_state(&state, path, &mut text_buffer);
    }
    apply_theme(
        state.borrow().dark_mode,
        &mut wind,
        &mut editor,
        &mut hidden_preview,
        &mut settings_win,
        &mut settings_hide_on_copy,
        &mut settings_dark,
        &mut opacity_label,
        &mut opacity_spinner,
        &mut settings_close,
    );
    wind.set_opacity(state.borrow().opacity);

    sync_window_title(&state, &mut wind);
    sync_menu_state(&state, &mut menu);
    apply_visibility_state(
        &state,
        &mut root,
        &mut visible_group,
        &mut hidden_group,
        &mut hidden_preview,
    );
    let _ = editor.take_focus();

    while app.wait() {
        if let Some(msg) = receiver.recv() {
            let mut effects = StateEffects::default();
            match msg {
                Msg::Open => {
                    if let Some(path) = choose_file() {
                        effects = load_file_into_state(&state, path, &mut text_buffer);
                    }
                }
                Msg::OpenPath(path) => {
                    effects = load_file_into_state(&state, path, &mut text_buffer);
                }
                Msg::Reload => {
                    let current_file = state.borrow().current_file.clone();
                    if let Some(path) = current_file {
                        effects = load_file_into_state(&state, path, &mut text_buffer);
                    }
                }
                Msg::OpenSettings => {
                    let s = state.borrow();
                    settings_hide_on_copy.set_value(s.hide_on_copy);
                    settings_dark.set_value(s.dark_mode);
                    opacity_spinner.set_value((s.opacity * 100.0).round());
                    drop(s);
                    settings_win.show();
                    settings_win.redraw();
                }
                Msg::Show => {
                    debug_log!("show requested");
                    effects = state.borrow_mut().apply(StateAction::Show);
                }

                Msg::Hide => {
                    debug_log!("temporary hide requested");
                    effects = state.borrow_mut().apply(StateAction::HideTemporarily);
                }
                Msg::Quit => app.quit(),
                Msg::ToggleAlwaysVisible => {
                    let enabled = !state.borrow().always_visible;
                    debug_log!("always_visible={enabled}");
                    effects = state
                        .borrow_mut()
                        .apply(StateAction::SetAlwaysVisible(enabled));
                }

                Msg::ToggleAutoShow => {
                    let enabled = !state.borrow().auto_show;
                    debug_log!("auto_show={enabled}");
                    effects = state.borrow_mut().apply(StateAction::SetAutoShow(enabled));
                }
                Msg::Tick => {
                    effects = state.borrow_mut().apply(StateAction::Tick);
                }
                Msg::Focused => {
                    let snapshot = state.borrow();
                    debug_log!(
                        "focused visibility={:?} auto_show={} always_visible={}",
                        snapshot.visibility,
                        snapshot.auto_show,
                        snapshot.always_visible
                    );
                    drop(snapshot);
                    effects = state.borrow_mut().apply(StateAction::Focused);
                }
                Msg::Unfocused => {
                    debug_log!("unfocused");
                    effects = state.borrow_mut().apply(StateAction::Unfocused);
                }
                Msg::DeferredHideOnUnfocus(expected_generation) => {
                    effects = state
                        .borrow_mut()
                        .apply(StateAction::DeferredHideOnUnfocus(expected_generation));
                }
                Msg::Copied => effects = state.borrow_mut().apply(StateAction::Copied),
                Msg::Resized => {
                    root.recalc();
                    root.redraw();
                    visible_group.redraw();
                    hidden_group.redraw();
                    hidden_preview.redraw();
                }
            }

            if let Some(hide_generation) = effects.schedule_hide_generation {
                let deferred_sender = sender;
                app::add_timeout3(FOCUS_HIDE_DELAY_SECONDS, move |_| {
                    deferred_sender.send(Msg::DeferredHideOnUnfocus(hide_generation));
                });
            }

            capture_window_state(&state, &wind);
            sync_settings(&state);
            if effects.sync_title {
                sync_window_title(&state, &mut wind);
            }
            if effects.sync_menu {
                sync_menu_state(&state, &mut menu);
            }
            if effects.apply_visibility {
                apply_visibility_state(
                    &state,
                    &mut root,
                    &mut visible_group,
                    &mut hidden_group,
                    &mut hidden_preview,
                );
            }
            if effects.redraw_hidden_preview {
                hidden_preview.redraw();
            }
            if effects.focus_editor && state.borrow().is_visible() {
                let _ = editor.take_focus();
            }
        }
    }
}

fn choose_file() -> Option<PathBuf> {
    let mut chooser = NativeFileChooser::new(FileDialogType::BrowseFile);
    chooser.set_title("Open");
    chooser.show();
    let file = chooser.filename();
    if file.as_os_str().is_empty() {
        None
    } else {
        Some(file)
    }
}

fn load_logo_image() -> Option<PngImage> {
    PngImage::from_data(LOGO_PNG).ok()
}

fn load_blurred_image() -> Option<PngImage> {
    PngImage::from_data(BLURRY_PNG).ok()
}

fn is_copy_event() -> bool {
    let state = app::event_state();
    let has_modifier = state.contains(Shortcut::Ctrl) || state.contains(Shortcut::Meta);
    let key = app::event_key();
    has_modifier && (key == Key::from_char('c') || key == Key::from_char('C'))
}

fn load_file_into_state(
    state: &Rc<RefCell<AppState>>,
    path: PathBuf,
    buffer: &mut TextBuffer,
) -> StateEffects {
    match read_document_text(&path) {
        Ok(text) => {
            buffer.set_text(&text);
            state.borrow_mut().apply(StateAction::FileLoaded { path })
        }
        Err(err) => state.borrow_mut().apply(StateAction::FileLoadFailed(err)),
    }
}

fn apply_visibility_state(
    state: &Rc<RefCell<AppState>>,
    root: &mut Flex,
    visible_group: &mut Group,
    hidden_group: &mut Group,
    hidden_preview: &mut Frame,
) {
    let visible = {
        let s = state.borrow();
        s.is_visible()
    };

    if visible {
        if !visible_group.visible() || hidden_group.visible() {
            hidden_group.hide();
            visible_group.show();
            root.recalc();
            root.redraw();
        }
        visible_group.redraw();
    } else {
        if visible_group.visible() || !hidden_group.visible() {
            visible_group.hide();
            hidden_group.show();
            root.recalc();
            root.redraw();
        }
        hidden_group.redraw();
        hidden_preview.redraw();
    }
}

fn sync_window_title(state: &Rc<RefCell<AppState>>, wind: &mut Window) {
    let title = {
        let s = state.borrow();
        if let Some(path) = &s.current_file {
            path.display().to_string()
        } else {
            "No file currently open".to_owned()
        }
    };
    if wind.label() != title {
        wind.set_label(&title);
    }
}

fn sync_menu_state(state: &Rc<RefCell<AppState>>, menu: &mut SysMenuBar) {
    let (always_visible, auto_show) = {
        let s = state.borrow();
        (s.always_visible, s.auto_show)
    };

    set_menu_toggle_by_label(menu, "Always Visible", always_visible);
    set_menu_toggle_by_label(menu, "Auto Show", auto_show);
    menu.redraw();
}

fn set_menu_toggle_by_label(menu: &mut SysMenuBar, target: &str, enabled: bool) {
    for i in 0..menu.size() {
        if let Some(mut item) = menu.at(i) {
            if let Some(label) = item.label() {
                let clean = label.replace('&', "").replace('\t', "");
                if clean == target {
                    if enabled {
                        item.set();
                    } else {
                        item.clear();
                    }
                    break;
                }
            }
        }
    }
}

fn sync_settings(state: &Rc<RefCell<AppState>>) {
    let mut s = state.borrow_mut();
    if !s.settings_dirty {
        return;
    }

    s.settings.always_visible = s.always_visible;
    s.settings.auto_show = s.auto_show;
    s.settings.hide_on_copy = s.hide_on_copy;
    s.settings.dark_mode = s.dark_mode;
    s.settings.opacity = s.opacity;
    save_saved_state(&s.settings);
    s.settings_dirty = false;
}

fn persist_current_settings(state: &Rc<RefCell<AppState>>) {
    let mut s = state.borrow_mut();
    s.settings.always_visible = s.always_visible;
    s.settings.auto_show = s.auto_show;
    s.settings.hide_on_copy = s.hide_on_copy;
    s.settings.dark_mode = s.dark_mode;
    s.settings.opacity = s.opacity;
    save_saved_state(&s.settings);
    s.settings_dirty = false;
}

fn decode_document_text(raw: &str) -> String {
    if raw.trim_start().starts_with("{\\rtf") {
        strip_basic_rtf(raw)
    } else {
        raw.to_owned()
    }
}

fn read_document_text(path: &PathBuf) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    if matches!(extension.as_deref(), Some("rtf")) {
        if let Ok(text) = convert_rtf_with_textutil(path) {
            return Ok(text);
        }
    }

    fs::read_to_string(path)
        .map(|raw| decode_document_text(&raw))
        .map_err(|err| format!("Could not open file: {err}"))
}

fn convert_rtf_with_textutil(path: &PathBuf) -> Result<String, String> {
    let output = Command::new("textutil")
        .args(["-convert", "txt", "-stdout"])
        .arg(path)
        .output()
        .map_err(|err| format!("textutil failed: {err}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }

    let text = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    Ok(text.trim_end().to_owned())
}

fn open_external_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };

    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };

    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };

    cmd.spawn().map(|_| ()).map_err(|err| err.to_string())
}

#[cfg(target_os = "macos")]
fn handle_open_file(path_ptr: *const std::os::raw::c_char) {
    if path_ptr.is_null() {
        return;
    }

    let path = unsafe { CStr::from_ptr(path_ptr) }
        .to_string_lossy()
        .into_owned();

    if let Some(sender) = OPEN_FILE_SENDER.get() {
        sender.send(Msg::OpenPath(PathBuf::from(path)));
    }
}

fn strip_basic_rtf(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    let mut skip_stack = vec![false];
    let mut skip_next_group = false;
    let destinations: HashSet<&'static str> = [
        "fonttbl",
        "colortbl",
        "stylesheet",
        "info",
        "pict",
        "expandedcolortbl",
        "generator",
    ]
    .into_iter()
    .collect();

    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                let parent_skip = *skip_stack.last().unwrap_or(&false);
                skip_stack.push(parent_skip || skip_next_group);
                skip_next_group = false;
            }
            '}' => {
                skip_stack.pop();
                if skip_stack.is_empty() {
                    skip_stack.push(false);
                }
            }
            '\\' => match chars.peek().copied() {
                Some('\\') | Some('{') | Some('}') => {
                    let escaped = chars.next().unwrap_or_default();
                    if !*skip_stack.last().unwrap_or(&false) {
                        out.push(escaped);
                    }
                }
                Some('\'') => {
                    chars.next();
                    let a = chars.next();
                    let b = chars.next();
                    if let (Some(a), Some(b)) = (a, b) {
                        if !*skip_stack.last().unwrap_or(&false) {
                            if let Ok(byte) = u8::from_str_radix(&format!("{a}{b}"), 16) {
                                out.push(byte as char);
                            }
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    skip_next_group = true;
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    let mut word = String::new();
                    while let Some(next) = chars.peek().copied() {
                        if next.is_ascii_alphabetic() {
                            word.push(next);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    while let Some(next) = chars.peek().copied() {
                        if next == '-' || next.is_ascii_digit() {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    if chars.peek() == Some(&' ') {
                        chars.next();
                    }

                    if destinations.contains(word.as_str()) {
                        if let Some(current) = skip_stack.last_mut() {
                            *current = true;
                        }
                        continue;
                    }

                    if *skip_stack.last().unwrap_or(&false) {
                        continue;
                    }

                    if word == "par" || word == "line" {
                        out.push('\n');
                    } else if word == "tab" {
                        out.push('\t');
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\r' => {}
            _ => {
                if !*skip_stack.last().unwrap_or(&false) {
                    out.push(ch);
                }
            }
        }
    }

    out.lines()
        .map(str::trim_end)
        .filter(|line| !looks_like_rtf_junk(line))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

fn looks_like_rtf_junk(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return true;
    }
    if lower.chars().all(|c| ";:".contains(c)) {
        return true;
    }
    lower == "helvetica" || lower == "times" || lower == "courier"
}

fn capture_window_state(state: &Rc<RefCell<AppState>>, wind: &Window) {
    let mut s = state.borrow_mut();
    let changed = s.settings.window_x != wind.x()
        || s.settings.window_y != wind.y()
        || s.settings.window_w != wind.w()
        || s.settings.window_h != wind.h();
    if changed {
        s.settings.window_x = wind.x();
        s.settings.window_y = wind.y();
        s.settings.window_w = wind.w();
        s.settings.window_h = wind.h();
        s.settings_dirty = true;
    }
}

fn apply_theme(
    dark_mode: bool,
    wind: &mut Window,
    editor: &mut TextDisplay,
    hidden_preview: &mut Frame,
    settings_win: &mut Window,
    settings_hide_on_copy: &mut CheckButton,
    settings_dark: &mut CheckButton,
    opacity_label: &mut Frame,
    opacity_spinner: &mut Spinner,
    settings_close: &mut Button,
) {
    if dark_mode {
        wind.set_color(Color::from_rgb(24, 26, 29));
        settings_win.set_color(Color::from_rgb(32, 35, 39));
        editor.set_color(Color::from_rgb(18, 20, 23));
        editor.set_text_color(Color::from_rgb(229, 232, 236));
        hidden_preview.set_color(Color::from_rgb(30, 33, 37));
        opacity_label.set_label_color(Color::from_rgb(229, 232, 236));
        settings_hide_on_copy.set_label_color(Color::from_rgb(229, 232, 236));
        settings_dark.set_label_color(Color::from_rgb(229, 232, 236));
        settings_close.set_label_color(Color::from_rgb(229, 232, 236));
        settings_close.set_color(Color::from_rgb(54, 59, 66));
        opacity_spinner.set_color(Color::from_rgb(54, 59, 66));
        opacity_spinner.set_text_color(Color::from_rgb(229, 232, 236));
    } else {
        wind.set_color(Color::White);
        settings_win.set_color(Color::from_rgb(246, 247, 249));
        editor.set_color(Color::White);
        editor.set_text_color(Color::Black);
        hidden_preview.set_color(Color::from_rgb(244, 247, 249));
        opacity_label.set_label_color(Color::Black);
        settings_hide_on_copy.set_label_color(Color::Black);
        settings_dark.set_label_color(Color::Black);
        settings_close.set_label_color(Color::Black);
        settings_close.set_color(Color::from_rgb(236, 238, 240));
        opacity_spinner.set_color(Color::White);
        opacity_spinner.set_text_color(Color::Black);
    }

    wind.redraw();
    settings_win.redraw();
    hidden_preview.redraw();
    editor.redraw();
}

fn initial_window_rect(settings: &SavedState) -> (i32, i32, i32, i32) {
    let width = settings.window_w;
    let height = if settings.window_h == LEGACY_DEFAULT_WINDOW_H
        && settings.window_w == DEFAULT_WINDOW_W
        && settings.last_file.is_none()
    {
        DEFAULT_WINDOW_H
    } else {
        settings.window_h
    };
    (settings.window_x, settings.window_y, width, height)
}

fn load_saved_state() -> SavedState {
    let Some(path) = settings_path() else {
        debug_log!("no settings path available");
        return SavedState::default();
    };

    let Ok(contents) = fs::read_to_string(&path) else {
        debug_log!("no settings file at {}", path.display());
        return SavedState::default();
    };

    match serde_json::from_str::<SavedState>(&contents) {
        Ok(state) => {
            debug_log!("loaded settings from {}", path.display());
            state
        }
        Err(err) => {
            debug_log!("failed to parse settings {}: {}", path.display(), err);
            SavedState::default()
        }
    }
}

fn save_saved_state(state: &SavedState) {
    let Some(path) = settings_path() else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(serialized) = serde_json::to_string_pretty(state) else {
        return;
    };
    let _ = fs::write(path, serialized);
}

fn settings_path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("com", "byteface", "blurred")?;
    Some(dirs.config_dir().join("settings.json"))
}
