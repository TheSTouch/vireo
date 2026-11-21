use fs::Fs;
use std::rc::Rc;

use gpui::{Action, AnyElement, App, prelude::*};
use settings::update_settings_file;
use ui::{KeyBinding, prelude::*};

use crate::{FocusInput, ShowSessions};

/// The "Get started" checklist shown in an empty conversation until the
/// person hides it.
pub fn render_checklist(cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let step = |number: &'static str, title: &'static str, detail: AnyElement| {
        h_flex()
            .items_start()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .size_5()
                    .rounded_full()
                    .border_1()
                    .border_color(colors.border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Label::new(number)
                            .size(LabelSize::XSmall)
                            .color(Color::Accent),
                    ),
            )
            .child(
                // Without a width limit the details run out of the card.
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(Label::new(title).size(LabelSize::Small))
                    .child(detail),
            )
    };
    let muted = |text: &'static str| {
        Label::new(text)
            .size(LabelSize::XSmall)
            .color(Color::Muted)
            .into_any_element()
    };
    v_flex()
        .w_full()
        .max_w(rems(26.))
        .mt_2()
        .p_3()
        .gap_2()
        .rounded_md()
        .border_1()
        .border_color(colors.border_variant)
        .bg(colors.elevated_surface_background)
        .child(Label::new("Get started with Claude Code").size(LabelSize::Small).weight(gpui::FontWeight::SEMIBOLD))
        .child(step(
            "1",
            "Open Claude Code",
            with_binding(
                "Click the spark in the dock",
                "Click the spark in the dock, or press",
                &FocusInput,
                cx,
            ),
        ))
        .child(step(
            "2",
            "Chat with Claude",
            muted("Type a message and press Enter. Use @ to mention files, or select code first to ask about it."),
        ))
        .child(step(
            "3",
            "Pick up past conversations",
            with_binding(
                "Use the history button",
                "Use the history button or",
                &ShowSessions,
                cx,
            ),
        ))
        .child(step(
            "4",
            "Choose how Claude works",
            muted("The mode button switches between asking first, editing automatically, and planning."),
        ))
        .child(
            h_flex().justify_end().child(
                Button::new("hide-onboarding", "Hide checklist")
                    .label_size(LabelSize::XSmall)
                    .color(Color::Muted)
                    .on_click(|_, _, cx| hide_onboarding(cx)),
            ),
        )
        .into_any_element()
}

/// A muted sentence ending in the action's key. The key is looked up in the
/// whole keymap: bindings such as Focus Input's belong to the editor's
/// context, which is not focused while the checklist shows.
fn with_binding(
    without_key: &'static str,
    with_key: &'static str,
    action: &dyn Action,
    cx: &App,
) -> AnyElement {
    let keystrokes = cx
        .key_bindings()
        .borrow()
        .bindings_for_action(action)
        .last()
        .map(|binding| Rc::<[_]>::from(binding.keystrokes()));
    let Some(keystrokes) = keystrokes else {
        return Label::new(format!("{without_key}."))
            .size(LabelSize::XSmall)
            .color(Color::Muted)
            .into_any_element();
    };
    h_flex()
        .gap_1()
        .child(
            Label::new(with_key)
                .size(LabelSize::XSmall)
                .color(Color::Muted),
        )
        .child(
            KeyBinding::from_keystrokes(keystrokes, KeyBinding::is_vim_mode(cx))
                .size(rems_from_px(11_f32)),
        )
        .into_any_element()
}

pub fn hide_onboarding(cx: &mut App) {
    let fs = <dyn Fs>::global(cx);
    update_settings_file(fs, cx, |settings, _| {
        settings.claude_code.get_or_insert_default().hide_onboarding = Some(true);
    });
}
