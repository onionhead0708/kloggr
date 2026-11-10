//! Choosing files extracted from an archive (klogg: the "Open file from
//! archive" file dialog on the temporary folder).

use std::path::PathBuf;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::{ActiveTheme, Disableable, Sizable, WindowExt, h_flex, v_flex};

/// The files the user chose.
pub struct Chosen(pub Vec<PathBuf>);

pub struct ArchivePicker {
    root: PathBuf,
    files: Vec<(PathBuf, bool)>,
}

impl EventEmitter<Chosen> for ArchivePicker {}

impl ArchivePicker {
    pub fn new(root: PathBuf, files: Vec<PathBuf>) -> Self {
        Self {
            root,
            files: files.into_iter().map(|f| (f, false)).collect(),
        }
    }

    pub fn chosen(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|(_, on)| *on)
            .map(|(f, _)| f.clone())
            .collect()
    }
}

impl Render for ArchivePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (border, muted) = (cx.theme().border, cx.theme().muted_foreground);
        let rows = self.files.iter().enumerate().map(|(i, (file, on))| {
            let shown = file
                .strip_prefix(&self.root)
                .unwrap_or(file)
                .display()
                .to_string();
            let size = std::fs::metadata(file).map_or(0, |m| m.len());
            h_flex()
                .gap_2()
                .child(
                    Checkbox::new(("archive-file", i))
                        .label(shown)
                        .checked(*on)
                        .on_click(cx.listener(move |this, on: &bool, _, cx| {
                            this.files[i].1 = *on;
                            cx.notify();
                        })),
                )
                .child(div().text_xs().text_color(muted).child(human_size(size)))
        });
        let none = self.chosen().is_empty();
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(format!("Extracted to {}", self.root.display())),
            )
            .child(
                v_flex()
                    .id("archive-files")
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .p_2()
                    .gap_1()
                    .children(rows),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("archive-cancel")
                            .label("Cancel")
                            .small()
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("archive-open")
                            .label("Open")
                            .small()
                            .primary()
                            .disabled(none)
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.emit(Chosen(this.chosen()));
                                window.close_dialog(cx);
                            })),
                    ),
            )
    }
}

fn human_size(bytes: u64) -> SharedString {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024. && unit + 1 < UNITS.len() {
        size /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B").into()
    } else {
        format!("{size:.1} {}", UNITS[unit]).into()
    }
}

/// Shows the picker; `chosen` gets the files when the user presses Open.
/// Keep the returned entity until then (see `key_capture::open`).
pub fn pick(
    root: PathBuf,
    files: Vec<PathBuf>,
    window: &mut Window,
    cx: &mut App,
    chosen: impl Fn(Vec<PathBuf>, &mut Window, &mut App) + 'static,
) -> Entity<ArchivePicker> {
    let picker = cx.new(|_| ArchivePicker::new(root, files));
    window
        .subscribe(&picker, cx, move |_, Chosen(files), window, cx| {
            chosen(files.clone(), window, cx)
        })
        .detach();
    let dialog_picker = picker.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Open file from archive")
            .w(px(620.))
            .child(dialog_picker.clone())
    });
    picker
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(10), "10 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(5 << 30), "5.0 GiB");
    }
}
