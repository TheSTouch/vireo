//! Prepared sessions: Markdown files in `assets/vireo/sessions`, each a
//! front matter block (`title`, `description`, optional `input`) and the
//! prompt Claude Code receives, where `{{input}}` stands for what the person
//! types in the session's box.

use std::path::Path;

/// Where prepared sessions live, relative to the root of Vireo's sources.
pub const SESSIONS_DIR: &str = "assets/vireo/sessions";

const BUILT_IN: &[(&str, &str)] = &[
    (
        "1-change-something.md",
        include_str!("../../../assets/vireo/sessions/1-change-something.md"),
    ),
    (
        "2-fix-a-bug.md",
        include_str!("../../../assets/vireo/sessions/2-fix-a-bug.md"),
    ),
    (
        "3-build-and-install.md",
        include_str!("../../../assets/vireo/sessions/3-build-and-install.md"),
    ),
    (
        "4-update.md",
        include_str!("../../../assets/vireo/sessions/4-update.md"),
    ),
];

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedSession {
    /// The file it came from, which also orders the list.
    pub file_name: String,
    pub title: String,
    pub description: String,
    /// The placeholder of the box the person describes their request in;
    /// `None` when the session takes no input.
    pub input: Option<String>,
    pub prompt: String,
}

impl PreparedSession {
    /// Reads a session file. Returns `None` when it has no front matter or no
    /// title, so a stray Markdown file in the folder is skipped.
    pub fn parse(file_name: &str, text: &str) -> Option<Self> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let rest = text.strip_prefix("---")?.trim_start_matches([' ', '\t']);
        let rest = rest
            .strip_prefix('\n')
            .or_else(|| rest.strip_prefix("\r\n"))?;
        let end = rest.find("\n---")?;
        let (front_matter, body) = rest.split_at(end);
        let body = body["\n---".len()..].trim_start_matches(['-']).trim();

        let mut title = None;
        let mut description = String::new();
        let mut input = None;
        for line in front_matter.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim().to_string();
            match key.trim() {
                "title" => title = Some(value),
                "description" => description = value,
                "input" => input = Some(value),
                _ => {}
            }
        }
        Some(Self {
            file_name: file_name.to_string(),
            title: title.filter(|title| !title.is_empty())?,
            description,
            input,
            prompt: body.to_string(),
        })
    }

    pub fn takes_input(&self) -> bool {
        self.input.is_some()
    }

    /// The prompt to put in Claude Code's message box.
    pub fn prompt_for(&self, input: &str) -> String {
        self.prompt.replace("{{input}}", input.trim())
    }
}

/// The sessions built into this copy of Vireo.
pub fn built_in() -> Vec<PreparedSession> {
    parse_all(
        BUILT_IN
            .iter()
            .map(|(name, text)| (name.to_string(), text.to_string())),
    )
}

/// The sessions in a checkout of Vireo's sources, so new or edited sessions
/// show without rebuilding Vireo. Falls back to the built-in ones when the
/// checkout has none.
pub fn load(sources: &Path) -> Vec<PreparedSession> {
    let dir = sources.join(SESSIONS_DIR);
    let files = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let text = std::fs::read_to_string(entry.path()).ok()?;
                    Some((name, text))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let sessions = parse_all(files.into_iter());
    if sessions.is_empty() {
        built_in()
    } else {
        sessions
    }
}

fn parse_all(files: impl Iterator<Item = (String, String)>) -> Vec<PreparedSession> {
    let mut sessions = files
        .filter_map(|(name, text)| {
            let session = PreparedSession::parse(&name, &text);
            if session.is_none() {
                log::warn!("skipping prepared session {name}: no front matter with a title");
            }
            session
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_front_matter_and_prompt() {
        let session = PreparedSession::parse(
            "a.md",
            "---\ntitle: Fix a bug\ndescription: Finds the cause: then fixes it.\ninput: What goes wrong?\n---\nThe bug:\n\n{{input}}\n",
        )
        .unwrap();
        assert_eq!(session.title, "Fix a bug");
        assert_eq!(session.description, "Finds the cause: then fixes it.");
        assert_eq!(session.input.as_deref(), Some("What goes wrong?"));
        assert_eq!(session.prompt, "The bug:\n\n{{input}}");
        assert_eq!(
            session.prompt_for("  tabs are too short \n"),
            "The bug:\n\ntabs are too short"
        );
    }

    #[test]
    fn sessions_without_input_or_title() {
        let session =
            PreparedSession::parse("b.md", "---\ntitle: Build\n---\nRun the build.").unwrap();
        assert!(!session.takes_input());
        assert_eq!(session.prompt_for(""), "Run the build.");

        assert_eq!(PreparedSession::parse("c.md", "No front matter"), None);
        assert_eq!(
            PreparedSession::parse("d.md", "---\ndescription: x\n---\nbody"),
            None
        );
    }

    #[test]
    fn built_in_sessions_are_valid_and_ordered() {
        let sessions = built_in();
        let titles = sessions
            .iter()
            .map(|session| session.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            [
                "Change something",
                "Fix a bug",
                "Build and install Vireo",
                "Update to the latest Vireo"
            ]
        );
        assert!(sessions[0].takes_input() && sessions[0].prompt.contains("{{input}}"));
        assert!(!sessions[2].takes_input());
    }

    #[test]
    fn checkout_sessions_win_over_built_in_ones() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), built_in());

        let sessions_dir = dir.path().join(SESSIONS_DIR);
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(
            sessions_dir.join("2-second.md"),
            "---\ntitle: Second\n---\nTwo",
        )
        .unwrap();
        std::fs::write(
            sessions_dir.join("1-first.md"),
            "---\ntitle: First\n---\nOne",
        )
        .unwrap();
        std::fs::write(sessions_dir.join("notes.txt"), "ignored").unwrap();
        let titles = load(dir.path())
            .into_iter()
            .map(|session| session.title)
            .collect::<Vec<_>>();
        assert_eq!(titles, ["First", "Second"]);
    }
}
