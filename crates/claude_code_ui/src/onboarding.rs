use fs::Fs;
use gpui::{AnyElement, App, prelude::*};
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
                v_flex()
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
            h_flex()
                .gap_1()
                .child(muted("Click the spark in the dock, or press"))
                .child(KeyBinding::for_action(&FocusInput, cx).size(rems_from_px(11_f32)))
                .into_any_element(),
        ))
        .child(step(
            "2",
            "Chat with Claude",
            muted("Type a message and press Enter. Use @ to mention files, or select code first to ask about it."),
        ))
        .child(step(
            "3",
            "Pick up past conversations",
            h_flex()
                .gap_1()
                .child(muted("Use the history button or"))
                .child(KeyBinding::for_action(&ShowSessions, cx).size(rems_from_px(11_f32)))
                .into_any_element(),
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

pub fn hide_onboarding(cx: &mut App) {
    let fs = <dyn Fs>::global(cx);
    update_settings_file(fs, cx, |settings, _| {
        settings.claude_code.get_or_insert_default().hide_onboarding = Some(true);
    });
}
