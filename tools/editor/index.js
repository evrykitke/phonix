/**
 * The rich text editor, as one global object.
 *
 * # What this file is, and is deliberately not
 *
 * It is a thin adapter over TipTap and nothing else. It owns no chrome: no
 * toolbar, no dialog, no button, no word of English. Everything a person sees
 * is drawn by `phonix_web::ui::editor` in Rust, styled with the application's
 * own tokens and translated through the catalog like every other control.
 *
 * That line is where it is for two reasons. A toolbar written here would be the
 * only part of the interface that does not answer to the theme or the language,
 * and it would be the only place a permission gate could be forgotten. And the
 * wasm side has to know the editor's state anyway - which marks are active,
 * whether the caret is in a table - so having it also own the buttons costs
 * nothing.
 *
 * # The surface
 *
 *     const handle = window.PhonixEditor.mount(element, {
 *         content: "<p>...</p>",
 *         label: "Terms",          // the writing area's accessible name
 *         editable: true,
 *         onChange(html, state) {},   // state is JSON, see `snapshot`
 *     });
 *
 *     handle.command("bold");                  // -> bool, whether it applied
 *     handle.command("link", "https://...");   // an empty argument unsets it
 *     handle.setContent("<p>...</p>");         // does not fire onChange
 *     handle.destroy();
 *
 * Every name crossing this boundary is snake_case - the commands, and the
 * fields of the state object - so the Rust side derives `Deserialize` without a
 * rename attribute per field and the two vocabularies read as one.
 *
 * Five functions and a string vocabulary, which is as small as the boundary
 * gets: every one of them is a `Reflect::get` and a `Function::call` on the
 * Rust side, and each one that does not exist is plumbing nobody has to write.
 *
 * # Building it
 *
 *     node tools/build-editor.mjs
 *
 * The output is committed, exactly as `tools/icons.txt` and its generator are.
 * Nobody needs node to build or deploy this application - only to change what
 * is in this file.
 */

import { Editor } from "@tiptap/core";
import { StarterKit } from "@tiptap/starter-kit";
import { TableKit } from "@tiptap/extension-table";

/**
 * Every command the toolbar can ask for, by the name Rust calls it.
 *
 * A closed table rather than a pass-through to `editor.chain()[name]()`: that
 * would make every method TipTap has part of this boundary, including the ones
 * that take options nobody has thought about. A name that is not here does
 * nothing and says so, which is a bug in Rust rather than a surprise here.
 */
const COMMANDS = {
    bold: (chain) => chain.toggleBold(),
    italic: (chain) => chain.toggleItalic(),
    underline: (chain) => chain.toggleUnderline(),
    strike: (chain) => chain.toggleStrike(),

    heading_2: (chain) => chain.toggleHeading({ level: 2 }),
    heading_3: (chain) => chain.toggleHeading({ level: 3 }),
    blockquote: (chain) => chain.toggleBlockquote(),
    bullet_list: (chain) => chain.toggleBulletList(),
    ordered_list: (chain) => chain.toggleOrderedList(),
    horizontal_rule: (chain) => chain.setHorizontalRule(),

    // `extendMarkRange` first, so that clicking anywhere in an existing link
    // and pressing the button edits the whole of it rather than splitting it
    // at the caret. An empty href is how the toolbar says "unlink".
    link: (chain, href) =>
        href
            ? chain.extendMarkRange("link").setLink({ href })
            : chain.extendMarkRange("link").unsetLink(),

    insert_table: (chain) =>
        chain.insertTable({ rows: 3, cols: 3, withHeaderRow: true }),
    add_column_after: (chain) => chain.addColumnAfter(),
    delete_column: (chain) => chain.deleteColumn(),
    add_row_after: (chain) => chain.addRowAfter(),
    delete_row: (chain) => chain.deleteRow(),
    delete_table: (chain) => chain.deleteTable(),

    undo: (chain) => chain.undo(),
    redo: (chain) => chain.redo(),
    // Everything off: marks removed and blocks flattened back to paragraphs.
    // The one command people reach for after pasting from a word processor.
    clear: (chain) => chain.unsetAllMarks().clearNodes(),
};

/**
 * What the toolbar needs to draw itself, as JSON.
 *
 * Sent on every transaction rather than asked for: the alternative is the Rust
 * side polling, and a toolbar that lights up a fraction of a second after the
 * caret moves reads as a lag in the typing.
 *
 * JSON rather than a structured object because it crosses into wasm, where a
 * string is one copy and an object is a `Reflect::get` per field.
 */
function snapshot(editor) {
    return JSON.stringify({
        bold: editor.isActive("bold"),
        italic: editor.isActive("italic"),
        underline: editor.isActive("underline"),
        strike: editor.isActive("strike"),
        heading_2: editor.isActive("heading", { level: 2 }),
        heading_3: editor.isActive("heading", { level: 3 }),
        blockquote: editor.isActive("blockquote"),
        bullet_list: editor.isActive("bulletList"),
        ordered_list: editor.isActive("orderedList"),
        link: editor.isActive("link"),
        // The href under the caret, so the link dialog opens holding what is
        // already there instead of an empty box.
        link_href: editor.getAttributes("link").href ?? "",
        in_table: editor.isActive("table"),
        can_undo: editor.can().undo(),
        can_redo: editor.can().redo(),
        empty: editor.isEmpty,
    });
}

/**
 * Put an editor in `element` and hand back the handle.
 *
 * `element` must be a node nothing else writes to. Leptos creates it, never
 * gives it children of its own, and never re-renders it - ProseMirror owns
 * everything below it from here on, and a framework patching those nodes is
 * the one way this arrangement breaks.
 */
function mount(element, options) {
    const settings = options ?? {};

    const editor = new Editor({
        element,
        extensions: [
            StarterKit.configure({
                // The link mark is part of StarterKit in TipTap 3; configured
                // here rather than added again, which would register the mark
                // twice and throw.
                //
                // `openOnClick: false` because this is an editor: clicking a
                // link should put the caret in it, not navigate away from the
                // form somebody is filling in.
                link: { openOnClick: false, autolink: true },
            }),
            TableKit.configure({ table: { resizable: true } }),
        ],
        content: settings.content ?? "",
        editable: settings.editable ?? true,
        // Off, and it matters: TipTap's default appends `class="tiptap"` and
        // ProseMirror's own attributes to the element wasm rendered. Naming the
        // class here instead keeps the styling in `style/main.css` with the
        // rest of the controls.
        editorProps: {
            attributes: {
                class: "editor-surface",
                // The contenteditable is the control, and it is created here
                // rather than by wasm - so this is the only place its
                // accessible name can be put. Without it a screen reader
                // announces an unlabelled edit region.
                ...(settings.label ? { "aria-label": settings.label } : {}),
            },
        },
        onUpdate: ({ editor }) => report(editor, settings),
        // Selection alone changes nothing about the document and everything
        // about the toolbar.
        onSelectionUpdate: ({ editor }) => report(editor, settings),
        onFocus: ({ editor }) => report(editor, settings),
        onBlur: ({ editor }) => report(editor, settings),
    });

    return {
        command(name, argument) {
            const build = COMMANDS[name];
            if (!build) {
                return false;
            }

            // `.focus()` on every command: a toolbar button takes focus from
            // the document when it is clicked, and a chain that runs without
            // giving it back leaves the caret nowhere and the next keystroke
            // outside the editor.
            return build(editor.chain().focus(), argument).run();
        },

        setContent(html) {
            // `emitUpdate: false`, because this is the form writing *into* the
            // editor - a reset, a reload, a draft restored. Emitting would call
            // back into Rust with the value Rust just set, and two signals
            // chasing each other is a render loop.
            editor.commands.setContent(html ?? "", { emitUpdate: false });
        },

        setEditable(editable) {
            editor.setEditable(editable === true);
        },

        getHTML() {
            return editor.getHTML();
        },

        destroy() {
            editor.destroy();
        },
    };
}

function report(editor, settings) {
    if (typeof settings.onChange === "function") {
        settings.onChange(editor.getHTML(), snapshot(editor));
    }
}

window.PhonixEditor = { mount };
