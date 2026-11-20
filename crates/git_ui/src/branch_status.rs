//! VS Code's branch and sync items at the left of the status bar: the current
//! branch with a `*` when the working tree has changes, then the sync button
//! with how far the branch is behind and ahead of its upstream.

use git::repository::UpstreamTrackingStatus;
use gpui::{
    Action, App, Context, Entity, IntoElement, ParentElement, Render, Subscription, Window,
};
use project::git_store::{GitStore, GitStoreEvent};
use settings::Settings as _;
use ui::{ButtonLike, Icon, IconName, IconSize, Label, LabelSize, Tooltip, prelude::*};
use workspace::{HideStatusItem, StatusBarSettings, StatusItemView, Workspace, item::ItemHandle};

/// The branch as VS Code labels it: its name, with `*` when there are
/// uncommitted changes.
pub fn branch_label(name: &str, has_changes: bool) -> String {
    if has_changes {
        format!("{name}*")
    } else {
        name.to_string()
    }
}

/// VS Code's sync label: commits to pull, then commits to push, or nothing
/// when the branch is level with its upstream.
pub fn sync_label(status: UpstreamTrackingStatus) -> Option<String> {
    (status.ahead > 0 || status.behind > 0).then(|| format!("{}↓ {}↑", status.behind, status.ahead))
}

pub struct BranchStatus {
    git_store: Entity<GitStore>,
    _subscription: Subscription,
}

impl BranchStatus {
    pub fn new(workspace: &Workspace, cx: &mut Context<Self>) -> Self {
        let git_store = workspace.project().read(cx).git_store().clone();
        let subscription = cx.subscribe(&git_store, |_, _, event, cx| {
            if matches!(
                event,
                GitStoreEvent::ActiveRepositoryChanged(_) | GitStoreEvent::RepositoryUpdated(..)
            ) {
                cx.notify();
            }
        });
        Self {
            git_store,
            _subscription: subscription,
        }
    }
}

impl Render for BranchStatus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hidden = || h_flex().id("branch-status");
        if !StatusBarSettings::get_global(cx).branch_button {
            return hidden();
        }
        let Some(repository) = self.git_store.read(cx).active_repository() else {
            return hidden();
        };
        let repository = repository.read(cx);
        let Some(branch) = repository.branch.as_ref() else {
            return hidden();
        };
        let has_changes = repository.status_summary().count > 0;
        let label = branch_label(branch.name(), has_changes);
        let tracking = branch
            .upstream
            .as_ref()
            .and_then(|upstream| upstream.tracking.status());

        h_flex()
            .id("branch-status")
            .gap_1()
            .child(
                ButtonLike::new("branch-status-branch")
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(IconName::GitBranch).size(IconSize::Small))
                            .child(Label::new(label).size(LabelSize::Small)),
                    )
                    .tooltip(|_window, cx| {
                        Tooltip::for_action("Checkout Branch", &zed_actions::git::Branch, cx)
                    })
                    .on_click(|_, window, cx| {
                        window.dispatch_action(zed_actions::git::Branch.boxed_clone(), cx)
                    }),
            )
            .when_some(tracking, |this, tracking| {
                let action: Box<dyn Action> = if tracking.behind > 0 {
                    git::Pull.boxed_clone()
                } else if tracking.ahead > 0 {
                    git::Push.boxed_clone()
                } else {
                    git::Fetch.boxed_clone()
                };
                let tooltip_action = action.boxed_clone();
                this.child(
                    ButtonLike::new("branch-status-sync")
                        .child(
                            h_flex()
                                .gap_1()
                                .child(Icon::new(IconName::Sync).size(IconSize::Small))
                                .children(
                                    sync_label(tracking)
                                        .map(|label| Label::new(label).size(LabelSize::Small)),
                                ),
                        )
                        .tooltip(move |_window, cx| {
                            Tooltip::for_action("Synchronize Changes", tooltip_action.as_ref(), cx)
                        })
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(action.boxed_clone(), cx)
                        }),
                )
            })
    }
}

impl StatusItemView for BranchStatus {
    fn set_active_pane_item(
        &mut self,
        _: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        Some(HideStatusItem::new(|settings| {
            settings.status_bar.get_or_insert_default().branch_button = Some(false);
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_and_sync_labels_read_like_vs_code() {
        assert_eq!(branch_label("main", false), "main");
        assert_eq!(branch_label("main", true), "main*");
        let status = |behind, ahead| UpstreamTrackingStatus { ahead, behind };
        assert_eq!(sync_label(status(0, 0)), None);
        assert_eq!(sync_label(status(2, 0)).as_deref(), Some("2↓ 0↑"));
        assert_eq!(sync_label(status(0, 3)).as_deref(), Some("0↓ 3↑"));
    }
}
