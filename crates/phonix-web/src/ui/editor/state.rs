//! What the editor is doing, and what it can be asked to do.
//!
//! Both halves of the vocabulary that crosses into JavaScript live here, in one
//! file, so that adding a button is one edit on each side of the boundary and
//! it is obvious when only one of them has been made.

use serde::Deserialize;

/// The editor's state at the caret, as the toolbar needs to draw it.
///
/// Arrives as JSON on every transaction - see `tools/editor/index.js` - so
/// every field is what the bundle sends under exactly that name.
///
/// `Default` is "an editor that is not there yet, or has just been torn down":
/// nothing active, nothing undoable, empty. A toolbar drawn from it is inert
/// and correct, which is what should be on screen while the bundle is still
/// being fetched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct EditorState {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub heading_2: bool,
    pub heading_3: bool,
    pub blockquote: bool,
    pub bullet_list: bool,
    pub ordered_list: bool,
    pub link: bool,
    /// The address under the caret, so the link box opens holding what is
    /// already there rather than empty.
    pub link_href: String,
    /// Whether the caret is inside a table, which is what puts the table row
    /// of the toolbar on screen at all.
    pub in_table: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub empty: bool,
}

impl EditorState {
    /// Whether `command` is currently applied at the caret.
    ///
    /// Only the toggles answer `true` here; an action like "insert a table" is
    /// not a state something can be in, and a button for one is never lit.
    pub const fn is_active(&self, command: Command) -> bool {
        match command {
            Command::Bold => self.bold,
            Command::Italic => self.italic,
            Command::Underline => self.underline,
            Command::Strike => self.strike,
            Command::Heading2 => self.heading_2,
            Command::Heading3 => self.heading_3,
            Command::Blockquote => self.blockquote,
            Command::BulletList => self.bullet_list,
            Command::OrderedList => self.ordered_list,
            Command::Link => self.link,
            _ => false,
        }
    }
}

/// Everything the toolbar can ask the editor to do.
///
/// A closed enum rather than a string at the call site: the names are the
/// bundle's, and a typo in one is a button that silently does nothing on a
/// screen nobody tests by hand. The table in [`Command::name`] is the only
/// place the two vocabularies meet, and `every_command_has_a_name` is what
/// stops a variant being added here and forgotten there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Bold,
    Italic,
    Underline,
    Strike,

    Heading2,
    Heading3,
    Blockquote,
    BulletList,
    OrderedList,
    HorizontalRule,

    /// Takes the address as its argument. An empty one unlinks.
    Link,

    InsertTable,
    AddColumnAfter,
    DeleteColumn,
    AddRowAfter,
    DeleteRow,
    DeleteTable,

    Undo,
    Redo,
    /// Marks off, blocks flattened. What people reach for after pasting out of
    /// a word processor.
    Clear,
}

impl Command {
    /// The name the bundle knows it by.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bold => "bold",
            Self::Italic => "italic",
            Self::Underline => "underline",
            Self::Strike => "strike",
            Self::Heading2 => "heading_2",
            Self::Heading3 => "heading_3",
            Self::Blockquote => "blockquote",
            Self::BulletList => "bullet_list",
            Self::OrderedList => "ordered_list",
            Self::HorizontalRule => "horizontal_rule",
            Self::Link => "link",
            Self::InsertTable => "insert_table",
            Self::AddColumnAfter => "add_column_after",
            Self::DeleteColumn => "delete_column",
            Self::AddRowAfter => "add_row_after",
            Self::DeleteRow => "delete_row",
            Self::DeleteTable => "delete_table",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Clear => "clear",
        }
    }

    /// Every command, for the tests and for nothing else.
    pub const ALL: &'static [Self] = &[
        Self::Bold,
        Self::Italic,
        Self::Underline,
        Self::Strike,
        Self::Heading2,
        Self::Heading3,
        Self::Blockquote,
        Self::BulletList,
        Self::OrderedList,
        Self::HorizontalRule,
        Self::Link,
        Self::InsertTable,
        Self::AddColumnAfter,
        Self::DeleteColumn,
        Self::AddRowAfter,
        Self::DeleteRow,
        Self::DeleteTable,
        Self::Undo,
        Self::Redo,
        Self::Clear,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The bundle's command table, read off disk.
    ///
    /// The test compiled into the binary cannot call JavaScript, so it reads
    /// the source instead. Crude, and it catches the failure that actually
    /// happens: a command added on one side of the boundary and not the other,
    /// which shows up as a button that does nothing rather than as an error.
    fn bundle_commands() -> BTreeSet<String> {
        let source = include_str!("../../../../../tools/editor/index.js");
        let table = source
            .split_once("const COMMANDS = {")
            .expect("the bundle declares a command table")
            .1
            .split_once("\n};")
            .expect("the command table is closed")
            .0;

        table
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let (name, rest) = line.split_once(':')?;
                // Only the entries, not the comments above them.
                rest.trim_start().starts_with('(').then(|| name.to_owned())
            })
            .collect()
    }

    #[test]
    fn every_command_has_a_name_the_bundle_knows() {
        let bundle = bundle_commands();
        assert!(!bundle.is_empty(), "read no commands out of the bundle");

        for command in Command::ALL {
            assert!(
                bundle.contains(command.name()),
                "{:?} is called {:?}, which the bundle does not implement",
                command,
                command.name()
            );
        }
    }

    #[test]
    fn the_bundle_implements_nothing_rust_cannot_reach() {
        let named: BTreeSet<&str> = Command::ALL.iter().map(|c| c.name()).collect();

        for command in bundle_commands() {
            assert!(
                named.contains(command.as_str()),
                "the bundle implements {command:?}, which no Command names"
            );
        }
    }

    #[test]
    fn names_are_unique() {
        let mut seen = BTreeSet::new();
        for command in Command::ALL {
            assert!(seen.insert(command.name()), "duplicate {:?}", command.name());
        }
        assert_eq!(seen.len(), Command::ALL.len());
    }

    #[test]
    fn a_missing_snapshot_reads_as_an_editor_doing_nothing() {
        let state: EditorState = serde_json::from_str("{}").expect("empty object");
        assert_eq!(state, EditorState::default());
        assert!(!state.is_active(Command::Bold));
    }

    #[test]
    fn a_snapshot_lights_the_buttons_it_names() {
        let state: EditorState =
            serde_json::from_str(r#"{"bold":true,"bullet_list":true,"in_table":true}"#)
                .expect("a snapshot");

        assert!(state.is_active(Command::Bold));
        assert!(state.is_active(Command::BulletList));
        assert!(state.in_table);
        assert!(!state.is_active(Command::Italic));
    }

    #[test]
    fn an_action_is_never_lit() {
        let state = EditorState {
            bold: true,
            ..EditorState::default()
        };

        // Nothing that *does* something reports itself as a state, however
        // much of the document it has just changed.
        assert!(!state.is_active(Command::InsertTable));
        assert!(!state.is_active(Command::Undo));
        assert!(!state.is_active(Command::Clear));
    }
}
