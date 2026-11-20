//! Vireo's addon UI API (extension API v0.9.0): commands, status bar items,
//! panels, views, editor decorations and workspace services.
//!
//! Commands are declared in `extension.toml` under `[commands.<id>]` and run
//! through [`crate::Extension::run_command`].

/// Editor decorations.
pub mod editor {
    pub use crate::wit::zed::extension::editor::*;
}

/// Status bar items, panels and views.
pub mod ui {
    pub use crate::wit::zed::extension::ui::*;

    /// Builds the content of a custom view as a tree, which
    /// [`Element::into_view_tree`] flattens into the [`ViewTree`] that
    /// [`crate::Extension::render_view`] returns.
    ///
    /// Element IDs must be unique within the view; events name the element
    /// they come from by its ID.
    #[derive(Clone, Debug)]
    pub struct Element {
        id: String,
        kind: ElementKind,
        children: Vec<Element>,
    }

    impl Element {
        fn new(id: impl Into<String>, kind: ElementKind) -> Self {
            Self {
                id: id.into(),
                kind,
                children: Vec::new(),
            }
        }

        fn with_children(
            id: impl Into<String>,
            kind: ElementKind,
            children: impl IntoIterator<Item = Element>,
        ) -> Self {
            Self {
                id: id.into(),
                kind,
                children: children.into_iter().collect(),
            }
        }

        /// Lays out `children` vertically.
        pub fn column(id: impl Into<String>, children: impl IntoIterator<Item = Element>) -> Self {
            Self::with_children(id, ElementKind::Column, children)
        }

        /// Lays out `children` horizontally.
        pub fn row(id: impl Into<String>, children: impl IntoIterator<Item = Element>) -> Self {
            Self::with_children(id, ElementKind::Row, children)
        }

        /// A column that scrolls vertically.
        pub fn scroll(id: impl Into<String>, children: impl IntoIterator<Item = Element>) -> Self {
            Self::with_children(id, ElementKind::Scroll, children)
        }

        /// A column of rows with list spacing.
        pub fn list(id: impl Into<String>, children: impl IntoIterator<Item = Element>) -> Self {
            Self::with_children(id, ElementKind::List, children)
        }

        pub fn label(id: impl Into<String>, text: impl Into<String>) -> Self {
            Self::new(id, ElementKind::Label(text.into()))
        }

        /// A Vireo icon, by name, such as `folder`.
        pub fn icon(id: impl Into<String>, name: impl Into<String>) -> Self {
            Self::new(id, ElementKind::Icon(name.into()))
        }

        /// A button. Clicking it sends [`ViewEvent::Click`].
        pub fn button(id: impl Into<String>, label: impl Into<String>) -> Self {
            Self::new(id, ElementKind::Button(label.into()))
        }

        /// A single-line text input. Editing it sends [`ViewEvent::Change`];
        /// Enter sends [`ViewEvent::Submit`].
        pub fn text_input(
            id: impl Into<String>,
            value: impl Into<String>,
            placeholder: Option<String>,
        ) -> Self {
            Self::new(
                id,
                ElementKind::TextInput(TextInputState {
                    value: value.into(),
                    placeholder,
                }),
            )
        }

        /// A checkbox. Toggling it sends [`ViewEvent::Toggle`].
        pub fn checkbox(id: impl Into<String>, label: impl Into<String>, checked: bool) -> Self {
            Self::new(
                id,
                ElementKind::Checkbox(CheckboxState {
                    label: label.into(),
                    checked,
                }),
            )
        }

        pub fn divider(id: impl Into<String>) -> Self {
            Self::new(id, ElementKind::Divider)
        }

        /// Flattens the tree, with this element as the root.
        pub fn into_view_tree(self) -> ViewTree {
            let mut elements = Vec::new();
            self.flatten(&mut elements);
            ViewTree { elements }
        }

        fn flatten(self, elements: &mut Vec<crate::wit::zed::extension::ui::Element>) -> u32 {
            let index = elements.len();
            elements.push(crate::wit::zed::extension::ui::Element {
                id: self.id,
                kind: self.kind,
                children: Vec::new(),
            });
            let children = self
                .children
                .into_iter()
                .map(|child| child.flatten(elements))
                .collect();
            elements[index].children = children;
            index as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ui::{Element, ElementKind};

    #[test]
    fn test_element_tree_flattens_with_the_root_first() {
        let tree = Element::column(
            "root",
            [
                Element::row(
                    "actions",
                    [Element::button("hello", "Hello"), Element::divider("gap")],
                ),
                Element::checkbox("shout", "Shout", true),
            ],
        )
        .into_view_tree();

        let summary: Vec<_> = tree
            .elements
            .iter()
            .map(|element| (element.id.as_str(), element.children.clone()))
            .collect();
        assert_eq!(
            summary,
            [
                ("root", vec![1, 4]),
                ("actions", vec![2, 3]),
                ("hello", vec![]),
                ("gap", vec![]),
                ("shout", vec![]),
            ]
        );
        assert!(matches!(&tree.elements[2].kind, ElementKind::Button(label) if label == "Hello"));
    }
}
