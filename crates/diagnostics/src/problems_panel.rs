//! VS Code's Problems view: the project diagnostics, docked in the bottom
//! panel beside the terminal.

use gpui::{
    Action, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement, Pixels, Render, Styled, Subscription, WeakEntity, Window, actions,
    px,
};
use project::{DiagnosticSummary, Project, project_settings::ProjectSettings};
use settings::Settings as _;
use ui::{IconName, v_flex};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
    item::Item as _,
};

use crate::{IncludeWarnings, ProjectDiagnosticsEditor};

actions!(
    problems_panel,
    [
        /// Opens the Problems panel and focuses it, or hides it when it has focus.
        ToggleFocus
    ]
);

const PROBLEMS_PANEL_KEY: &str = "ProblemsPanel";

pub(crate) fn init(cx: &mut App) {
    cx.observe_new(
        |workspace: &mut Workspace, window: Option<&mut Window>, cx: &mut Context<Workspace>| {
            workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
                workspace.toggle_panel_focus::<ProblemsPanel>(window, cx);
            });
            let Some(window) = window else {
                return;
            };
            let workspace_handle = cx.entity().downgrade();
            let panel = cx.new(|cx| ProblemsPanel::new(workspace, workspace_handle, window, cx));
            workspace.add_panel(panel, window, cx);
        },
    )
    .detach();
}

/// The tab label VS Code shows next to "Problems": errors plus warnings, or
/// nothing when there are none.
pub(crate) fn problem_count_label(summary: DiagnosticSummary) -> Option<String> {
    let count = summary.error_count + summary.warning_count;
    (count > 0).then(|| count.to_string())
}

pub struct ProblemsPanel {
    diagnostics: Entity<ProjectDiagnosticsEditor>,
    project: Entity<Project>,
    _subscriptions: Vec<Subscription>,
}

impl ProblemsPanel {
    fn new(
        workspace: &mut Workspace,
        workspace_handle: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project = workspace.project().clone();
        let include_warnings = match cx.try_global::<IncludeWarnings>() {
            Some(include_warnings) => include_warnings.0,
            None => ProjectSettings::get_global(cx).diagnostics.include_warnings,
        };
        let diagnostics = cx.new(|cx| {
            ProjectDiagnosticsEditor::new(
                include_warnings,
                project.clone(),
                workspace_handle,
                window,
                cx,
            )
        });
        // The diagnostics view expects to live in the workspace, so that
        // opening a problem navigates there.
        diagnostics.update(cx, |diagnostics, cx| {
            diagnostics.added_to_workspace(workspace, window, cx)
        });
        let subscriptions = vec![
            cx.subscribe(&project, |_, _, event, cx| {
                if matches!(event, project::Event::DiagnosticsUpdated { .. }) {
                    cx.notify();
                }
            }),
            cx.observe(&diagnostics, |_, _, cx| cx.notify()),
        ];
        Self {
            diagnostics,
            project,
            _subscriptions: subscriptions,
        }
    }
}

impl EventEmitter<PanelEvent> for ProblemsPanel {}

impl Focusable for ProblemsPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.diagnostics.focus_handle(cx)
    }
}

impl Render for ProblemsPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().size_full().child(self.diagnostics.clone())
    }
}

impl Panel for ProblemsPanel {
    fn persistent_name() -> &'static str {
        PROBLEMS_PANEL_KEY
    }

    fn panel_key() -> &'static str {
        PROBLEMS_PANEL_KEY
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Bottom
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        position == DockPosition::Bottom
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(300.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Warning)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Problems")
    }

    fn icon_label(&self, _window: &Window, cx: &App) -> Option<String> {
        problem_count_label(self.project.read(cx).diagnostic_summary(false, cx))
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problem_count_adds_errors_and_warnings() {
        let summary = |error_count, warning_count| DiagnosticSummary {
            error_count,
            warning_count,
        };
        assert_eq!(problem_count_label(summary(0, 0)), None);
        assert_eq!(problem_count_label(summary(2, 3)).as_deref(), Some("5"));
        assert_eq!(problem_count_label(summary(0, 1)).as_deref(), Some("1"));
    }
}
