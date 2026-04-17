mod document;

use std::cell::RefCell;
#[cfg(target_os = "macos")]
use std::ffi::CStr;
use std::fs;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;

use directories::ProjectDirs;
use document::read_document_text;
#[cfg(target_os = "macos")]
use fltk::menu::{MacAppMenu, MenuItem, WindowMenuStyle};
use fltk::{
    app,
    button::{Button, CheckButton},
    dialog::{FileDialogType, NativeFileChooser},
    draw,
    enums::{Align, Color, Event, Font, FrameType, Key, Shortcut},
    frame::Frame,
    group::{Flex, Group},
    image::PngImage,
    menu::{MenuFlag, SysMenuBar},
    misc::Spinner,
    output::MultilineOutput,
    prelude::*,
    window::Window,
};
use serde::{Deserialize, Serialize};

const NOTICE_TICK_SECONDS: f64 = 0.1;
const FOCUS_HIDE_DELAY_SECONDS: f64 = 0.075;
const DEFAULT_OPACITY: f64 = 1.0;
const DEFAULT_WINDOW_W: i32 = 500;
const DEFAULT_WINDOW_H: i32 = 500;
const MAX_RECENT_FILES: usize = 8;
const BLURRY_PNG: &[u8] = include_bytes!("../blurry.png");
const APP_SCHEME: Option<app::Scheme> = None;

static OPEN_FILE_SENDER: OnceLock<app::Sender<Msg>> = OnceLock::new();

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct SavedState {
    always_visible: bool,
    auto_show: bool,
    hide_on_copy: bool,
    dark_mode: bool,
    opacity: f64,
    last_file: Option<PathBuf>,
    recent_files: Vec<PathBuf>,
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
            recent_files: Vec::new(),
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
                self.push_recent_file();
                self.visibility = VisibilityState::Visible;
                self.copied_notice_until = None;
                self.last_error = None;
                self.settings_dirty = true;
                StateEffects {
                    sync_title: true,
                    sync_menu: true,
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

    fn push_recent_file(&mut self) {
        let Some(current) = self.current_file.clone() else {
            return;
        };

        self.settings.recent_files.retain(|path| path != &current);
        self.settings.recent_files.insert(0, current);
        self.settings.recent_files.truncate(MAX_RECENT_FILES);
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
    ToggleVisibility,
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
    let app = app::App::default();
    if let Some(scheme) = APP_SCHEME {
        app::set_scheme(scheme);
    }
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
    rebuild_menu(&state, &mut menu, sender);
    root.fixed(&menu, 30);

    let mut visible_group = Group::default_fill();
    let mut visible_flex = Flex::default_fill().column();
    visible_flex.set_margin(0);
    visible_flex.set_pad(0);

    let mut editor = MultilineOutput::default_fill();
    editor.set_text_font(Font::Courier);
    editor.set_text_size(16);
    editor.set_frame(FrameType::FlatBox);
    editor.set_color(Color::White);
    editor.set_readonly(true);
    editor.set_wrap(true);
    editor.clear_visible_focus();
    editor.set_cursor_color(Color::White);
    let mut hide_button_row = Flex::default().row();
    hide_button_row.set_margin(12);
    hide_button_row.set_pad(0);
    let visible_left_spacer = Frame::default();
    let mut hide_btn = Button::default().with_label("🙈 Hide");
    hide_btn.clear_visible_focus();
    let visible_right_spacer = Frame::default();
    hide_button_row.fixed(&visible_left_spacer, 0);
    hide_button_row.fixed(&hide_btn, 100);
    hide_button_row.fixed(&visible_right_spacer, 0);
    hide_button_row.end();
    visible_flex.fixed(&hide_button_row, 56);
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
    let mut show_btn = Button::default().with_label("👀 Show");
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

    hide_btn.emit(sender, Msg::Hide);
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

    let editor_sender = sender;
    editor.handle(move |_ed, ev| match ev {
        Event::Push if app::event_mouse_button() == app::MouseButton::Right => true,
        Event::Released if app::event_mouse_button() == app::MouseButton::Right => true,
        Event::KeyDown | Event::Shortcut => {
            if is_toggle_visibility_event() {
                editor_sender.send(Msg::ToggleVisibility);
                return true;
            }
            false
        }
        _ => false,
    });

    let resize_pending = Rc::new(RefCell::new(false));
    let resize_pending_for_handle = resize_pending.clone();
    let sender_for_focus = sender;
    let mut editor_for_shortcuts = editor.clone();
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
            let mut pending = resize_pending_for_handle.borrow_mut();
            if !*pending {
                *pending = true;
                sender_for_focus.send(Msg::Resized);
            }
            false
        }
        Event::KeyDown | Event::Shortcut => {
            if is_toggle_visibility_event() {
                sender_for_focus.send(Msg::ToggleVisibility);
                true
            } else if is_copy_event()
                && editor_for_shortcuts.position() != editor_for_shortcuts.mark()
            {
                let _ = editor_for_shortcuts.copy();
                sender_for_focus.send(Msg::Copied);
                true
            } else {
                false
            }
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
        load_file_into_state(&state, path, &mut editor);
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
    rebuild_menu(&state, &mut menu, sender);
    apply_visibility_state(
        &state,
        &mut root,
        &mut visible_group,
        &mut hidden_group,
        &mut hidden_preview,
    );
    let _ = wind.take_focus();

    while app.wait() {
        if let Some(msg) = receiver.recv() {
            let mut effects = StateEffects::default();
            match msg {
                Msg::Open => {
                    if let Some(path) = choose_file() {
                        effects = load_file_into_state(&state, path, &mut editor);
                    }
                }
                Msg::OpenPath(path) => {
                    effects = load_file_into_state(&state, path, &mut editor);
                }
                Msg::Reload => {
                    let current_file = state.borrow().current_file.clone();
                    if let Some(path) = current_file {
                        effects = load_file_into_state(&state, path, &mut editor);
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
                Msg::Show => effects = state.borrow_mut().apply(StateAction::Show),
                Msg::Hide => effects = state.borrow_mut().apply(StateAction::HideTemporarily),
                Msg::ToggleVisibility => effects = toggle_visibility(&state),
                Msg::Quit => app.quit(),
                Msg::ToggleAlwaysVisible => {
                    let enabled = !state.borrow().always_visible;
                    effects = state
                        .borrow_mut()
                        .apply(StateAction::SetAlwaysVisible(enabled));
                }

                Msg::ToggleAutoShow => {
                    let enabled = !state.borrow().auto_show;
                    effects = state.borrow_mut().apply(StateAction::SetAutoShow(enabled));
                }
                Msg::Tick => effects = state.borrow_mut().apply(StateAction::Tick),
                Msg::Focused => effects = state.borrow_mut().apply(StateAction::Focused),
                Msg::Unfocused => effects = state.borrow_mut().apply(StateAction::Unfocused),
                Msg::DeferredHideOnUnfocus(expected_generation) => {
                    effects = state
                        .borrow_mut()
                        .apply(StateAction::DeferredHideOnUnfocus(expected_generation));
                }
                Msg::Copied => effects = state.borrow_mut().apply(StateAction::Copied),
                Msg::Resized => {
                    *resize_pending.borrow_mut() = false;
                    effects.apply_visibility = true;
                    effects.redraw_hidden_preview = true;
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
                rebuild_menu(&state, &mut menu, sender);
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
                let _ = wind.take_focus();
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

fn load_blurred_image() -> Option<PngImage> {
    PngImage::from_data(BLURRY_PNG).ok()
}

fn is_copy_event() -> bool {
    let state = app::event_state();
    let has_modifier = state.contains(Shortcut::Ctrl) || state.contains(Shortcut::Meta);
    let key = app::event_key();
    has_modifier && (key == Key::from_char('c') || key == Key::from_char('C'))
}

fn is_toggle_visibility_event() -> bool {
    let state = app::event_state();
    let has_modifier = state.contains(Shortcut::Ctrl)
        || state.contains(Shortcut::Meta)
        || state.contains(Shortcut::Alt);
    !has_modifier && app::event_key() == Key::from_char(' ')
}

fn load_file_into_state(
    state: &Rc<RefCell<AppState>>,
    path: PathBuf,
    editor: &mut MultilineOutput,
) -> StateEffects {
    match read_document_text(&path) {
        Ok(text) => {
            editor.set_value(&text);
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
        hidden_group.hide();
        visible_group.show();
        root.recalc();
        root.redraw();
        visible_group.redraw();
    } else {
        visible_group.hide();
        hidden_group.show();
        root.recalc();
        root.redraw();
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

fn rebuild_menu(state: &Rc<RefCell<AppState>>, menu: &mut SysMenuBar, sender: app::Sender<Msg>) {
    let (always_visible, auto_show, recent_files) = {
        let s = state.borrow();
        (
            s.always_visible,
            s.auto_show,
            s.settings.recent_files.clone(),
        )
    };

    menu.clear();
    menu.add_emit(
        "&File/Open...\t",
        Shortcut::Ctrl | 'o',
        MenuFlag::Normal,
        sender,
        Msg::Open,
    );

    if recent_files.is_empty() {
        menu.add(
            "&File/Open Recent/(Empty)",
            Shortcut::None,
            MenuFlag::Inactive,
            |_| {},
        );
    } else {
        for (index, path) in recent_files.into_iter().enumerate() {
            let label = format!(
                "&File/Open Recent/{}",
                format_recent_menu_label(index, &path)
            );
            menu.add_emit(
                &label,
                Shortcut::None,
                MenuFlag::Normal,
                sender,
                Msg::OpenPath(path),
            );
        }
    }

    menu.add_emit(
        "&File/Reload\t",
        Shortcut::Ctrl | 'r',
        MenuFlag::Normal,
        sender,
        Msg::Reload,
    );
    menu.add_emit(
        "&File/Quit\t",
        Shortcut::Ctrl | 'q',
        MenuFlag::Normal,
        sender,
        Msg::Quit,
    );

    let always_visible_flags = if always_visible {
        MenuFlag::Toggle | MenuFlag::Value
    } else {
        MenuFlag::Toggle
    };
    menu.add_emit(
        "&Options/Always Visible\t",
        Shortcut::None,
        always_visible_flags,
        sender,
        Msg::ToggleAlwaysVisible,
    );

    let auto_show_flags = if auto_show {
        MenuFlag::Toggle | MenuFlag::Value
    } else {
        MenuFlag::Toggle
    };
    menu.add_emit(
        "&Options/Auto Show\t",
        Shortcut::None,
        auto_show_flags,
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

    menu.redraw();
}

fn format_recent_menu_label(index: usize, path: &PathBuf) -> String {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled");
    let folder_name = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str());

    let label = if let Some(folder) = folder_name {
        format!("{}. {} ({})", index + 1, file_name, folder)
    } else {
        format!("{}. {}", index + 1, file_name)
    };

    sanitize_menu_label(&label)
}

fn sanitize_menu_label(label: &str) -> String {
    label
        .replace('&', "&&")
        .replace('/', " / ")
        .replace('\\', " ")
}

fn sync_settings(state: &Rc<RefCell<AppState>>) {
    persist_settings_if_needed(state, true);
}

fn persist_current_settings(state: &Rc<RefCell<AppState>>) {
    persist_settings_if_needed(state, false);
}

fn persist_settings_if_needed(state: &Rc<RefCell<AppState>>, only_if_dirty: bool) {
    let mut s = state.borrow_mut();
    if only_if_dirty && !s.settings_dirty {
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

fn toggle_visibility(state: &Rc<RefCell<AppState>>) -> StateEffects {
    if state.borrow().is_visible() {
        state.borrow_mut().apply(StateAction::HideTemporarily)
    } else {
        state.borrow_mut().apply(StateAction::Show)
    }
}

#[cfg(target_os = "macos")]
fn open_external_url(url: &str) -> Result<(), String> {
    Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
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
    editor: &mut MultilineOutput,
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
        editor.set_cursor_color(Color::from_rgb(18, 20, 23));
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
        editor.set_cursor_color(Color::White);
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
    (
        settings.window_x,
        settings.window_y,
        settings.window_w,
        settings.window_h,
    )
}

fn load_saved_state() -> SavedState {
    let Some(path) = settings_path() else {
        return SavedState::default();
    };

    let Ok(contents) = fs::read_to_string(&path) else {
        return SavedState::default();
    };

    match serde_json::from_str::<SavedState>(&contents) {
        Ok(state) => state,
        Err(_) => SavedState::default(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_state() -> AppState {
        let settings = SavedState::default();
        AppState {
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

    #[test]
    fn file_loaded_updates_last_file_recent_files_and_effects() {
        let mut state = test_state();
        let first = PathBuf::from("/tmp/one.txt");
        let second = PathBuf::from("/tmp/two.txt");

        state.apply(StateAction::FileLoaded {
            path: first.clone(),
        });
        let effects = state.apply(StateAction::FileLoaded {
            path: second.clone(),
        });

        assert_eq!(state.current_file, Some(second.clone()));
        assert_eq!(state.settings.last_file, Some(second.clone()));
        assert_eq!(state.settings.recent_files, vec![second, first]);
        assert!(effects.sync_title);
        assert!(effects.sync_menu);
        assert!(effects.apply_visibility);
        assert!(effects.focus_editor);
    }

    #[test]
    fn recent_files_are_deduped_and_truncated() {
        let mut state = test_state();

        for index in 0..(MAX_RECENT_FILES + 2) {
            state.apply(StateAction::FileLoaded {
                path: PathBuf::from(format!("/tmp/file-{index}.txt")),
            });
        }

        state.apply(StateAction::FileLoaded {
            path: PathBuf::from("/tmp/file-3.txt"),
        });

        assert_eq!(state.settings.recent_files.len(), MAX_RECENT_FILES);
        assert_eq!(
            state.settings.recent_files.first(),
            Some(&PathBuf::from("/tmp/file-3.txt"))
        );
        assert_eq!(
            state
                .settings
                .recent_files
                .iter()
                .filter(|path| **path == PathBuf::from("/tmp/file-3.txt"))
                .count(),
            1
        );
    }

    #[test]
    fn temporary_hide_ignores_first_focus_then_allows_focus_loss_cycle() {
        let mut state = test_state();

        state.apply(StateAction::HideTemporarily);
        assert_eq!(
            state.visibility,
            VisibilityState::Hidden(HideReason::Temporary {
                ignored_focus_after_hide: false
            })
        );

        let focus_effects = state.apply(StateAction::Focused);
        assert_eq!(
            state.visibility,
            VisibilityState::Hidden(HideReason::Temporary {
                ignored_focus_after_hide: true
            })
        );
        assert!(!focus_effects.apply_visibility);

        let unfocus_effects = state.apply(StateAction::Unfocused);
        assert_eq!(
            state.visibility,
            VisibilityState::Hidden(HideReason::FocusLoss)
        );
        assert_eq!(unfocus_effects.schedule_hide_generation, None);

        let refocus_effects = state.apply(StateAction::Focused);
        assert_eq!(state.visibility, VisibilityState::Visible);
        assert!(refocus_effects.apply_visibility);
        assert!(refocus_effects.focus_editor);
    }

    #[test]
    fn auto_show_disabled_keeps_window_hidden_on_focus() {
        let mut state = test_state();
        state.apply(StateAction::SetAutoShow(false));
        state.apply(StateAction::HideTemporarily);
        state.apply(StateAction::Focused);
        state.apply(StateAction::Unfocused);

        let effects = state.apply(StateAction::Focused);

        assert_eq!(
            state.visibility,
            VisibilityState::Hidden(HideReason::FocusLoss)
        );
        assert!(!effects.apply_visibility);
    }

    #[test]
    fn unfocus_while_visible_schedules_deferred_hide() {
        let mut state = test_state();

        let effects = state.apply(StateAction::Unfocused);

        assert_eq!(effects.schedule_hide_generation, Some(1));
        assert_eq!(state.focus_generation, 1);
    }

    #[test]
    fn deferred_hide_only_applies_for_matching_generation() {
        let mut state = test_state();
        let scheduled = state.apply(StateAction::Unfocused).schedule_hide_generation;

        let wrong_generation_effects = state.apply(StateAction::DeferredHideOnUnfocus(999));
        assert_eq!(state.visibility, VisibilityState::Visible);
        assert!(!wrong_generation_effects.apply_visibility);

        let correct_generation_effects =
            state.apply(StateAction::DeferredHideOnUnfocus(scheduled.unwrap()));
        assert_eq!(
            state.visibility,
            VisibilityState::Hidden(HideReason::FocusLoss)
        );
        assert!(correct_generation_effects.apply_visibility);
    }

    #[test]
    fn copy_hides_with_notice_and_tick_clears_notice() {
        let mut state = test_state();

        let copy_effects = state.apply(StateAction::Copied);
        assert_eq!(
            state.visibility,
            VisibilityState::Hidden(HideReason::CopyNotice)
        );
        assert!(state.copied_notice_until.is_some());
        assert!(copy_effects.apply_visibility);
        assert!(copy_effects.redraw_hidden_preview);

        state.copied_notice_until = Some(Instant::now() - Duration::from_millis(10));
        let tick_effects = state.apply(StateAction::Tick);
        assert_eq!(state.copied_notice_until, None);
        assert!(tick_effects.redraw_hidden_preview);
    }

    #[test]
    fn copy_does_not_hide_when_hide_on_copy_disabled() {
        let mut state = test_state();
        state.hide_on_copy = false;

        let effects = state.apply(StateAction::Copied);

        assert_eq!(state.visibility, VisibilityState::Visible);
        assert!(state.copied_notice_until.is_none());
        assert!(!effects.apply_visibility);
    }

    #[test]
    fn show_clears_copy_notice_and_restores_visibility() {
        let mut state = test_state();
        state.apply(StateAction::Copied);

        let effects = state.apply(StateAction::Show);

        assert_eq!(state.visibility, VisibilityState::Visible);
        assert_eq!(state.copied_notice_until, None);
        assert!(effects.apply_visibility);
        assert!(effects.focus_editor);
    }

    #[test]
    fn always_visible_forces_show_and_blocks_hide() {
        let mut state = test_state();
        state.apply(StateAction::HideTemporarily);

        let effects = state.apply(StateAction::SetAlwaysVisible(true));

        assert!(state.always_visible);
        assert_eq!(state.visibility, VisibilityState::Visible);
        assert!(effects.sync_menu);
        assert!(effects.apply_visibility);
        assert!(effects.focus_editor);

        let hide_effects = state.apply(StateAction::HideTemporarily);
        assert_eq!(state.visibility, VisibilityState::Visible);
        assert!(!hide_effects.apply_visibility);
    }

    #[test]
    fn toggle_actions_mark_settings_dirty() {
        let mut state = test_state();

        let always_effects = state.apply(StateAction::SetAlwaysVisible(true));
        assert!(state.settings_dirty);
        assert!(always_effects.sync_menu);

        state.settings_dirty = false;
        let auto_effects = state.apply(StateAction::SetAutoShow(false));
        assert!(state.settings_dirty);
        assert!(auto_effects.sync_menu);
        assert!(!state.auto_show);
    }
}
