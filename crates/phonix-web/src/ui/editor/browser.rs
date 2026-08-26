//! The half of the editor that only exists in a browser.
//!
//! # Why any of this is hand-written
//!
//! `#[wasm_bindgen] extern "C"` would be shorter, and it binds a name that has
//! to exist when the glue module is evaluated. The bundle here is fetched on
//! demand - it is 130 KiB gzipped, and most pages have no editor on them - so
//! at the moment the glue would want `window.PhonixEditor`, there is no such
//! thing. `Reflect` looks the name up when it is used, which is the only order
//! that works.
//!
//! # Nothing here may panic
//!
//! Every lookup is a `Result`, and every failure returns rather than unwrapping.
//! A panic in wasm is not an error message: it poisons the runtime and *every*
//! handler on the page stops responding - see [`crate::recovery`]. An editor
//! that fails to load should be a form with no editor in it, which is
//! recoverable, and never a page that has stopped listening.
//!
//! The whole module is `hydrate`-only. The server renders the shell, the mount
//! point stays empty, and the editor is put into it after hydration - so there
//! is nothing here for an SSR build to call. See [`super`] for the empty
//! counterparts.
//!
//! Two layers, and the split is worth keeping: [`Editor`] is the boundary with
//! JavaScript and knows nothing about Leptos, and [`Dispatch`] is the wiring
//! that owns one for as long as a component is on screen.

use std::cell::RefCell;

use js_sys::{Function, Object, Promise, Reflect};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Element, HtmlScriptElement};

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use super::bundle::EDITOR_SRC;
use super::state::{Command, EditorState};

/// The global the bundle installs itself as.
const GLOBAL: &str = "PhonixEditor";

thread_local! {
    /// The one in-flight (or settled) load of the bundle.
    ///
    /// Two editors on one page - a form with two rich text fields - would
    /// otherwise each append a `<script>` and each fetch 130 KiB. A settled
    /// promise resolves immediately, so the second, third and hundredth caller
    /// all await the first load rather than starting another.
    ///
    /// `thread_local!` is a plain global here: wasm has one thread, and this
    /// is the shape `std` gives a global that is not `Sync`.
    static LOADING: RefCell<Option<Promise>> = const { RefCell::new(None) };
}

/// Make sure `window.PhonixEditor` exists, fetching the bundle if it does not.
pub async fn load() -> Result<(), JsValue> {
    if global().is_ok() {
        return Ok(());
    }

    let promise = LOADING.with(|cell| {
        if let Some(existing) = cell.borrow().as_ref() {
            return existing.clone();
        }

        let promise = fetch_bundle();
        *cell.borrow_mut() = Some(promise.clone());
        promise
    });

    JsFuture::from(promise).await.map(|_| ())
}

/// Append the `<script>` and resolve when it has run.
fn fetch_bundle() -> Promise {
    Promise::new(&mut |resolve, reject| {
        let outcome = (|| -> Result<(), JsValue> {
            let document = web_sys::window()
                .and_then(|window| window.document())
                .ok_or_else(|| JsValue::from_str("no document"))?;

            let script: HtmlScriptElement =
                document.create_element("script")?.unchecked_into();
            script.set_src(EDITOR_SRC);
            // Defaulted to true by `async` on a dynamically inserted script,
            // and stated anyway: there is exactly one of these and nothing
            // depends on its order relative to anything else.
            script.set_async(true);
            script.set_onload(Some(&resolve));
            script.set_onerror(Some(&reject));

            let head = document
                .head()
                .ok_or_else(|| JsValue::from_str("no head"))?;
            head.append_child(&script)?;
            Ok(())
        })();

        // A failure to even insert the tag has to settle the promise, or every
        // caller awaits forever - which on this page means a spinner that never
        // stops rather than an error somebody can act on.
        if let Err(err) = outcome
            && let Err(rejection) = reject.call1(&JsValue::NULL, &err)
        {
            web_sys::console::error_1(&rejection);
        }
    })
}

fn global() -> Result<Object, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let global = Reflect::get(&window, &JsValue::from_str(GLOBAL))?;

    if global.is_undefined() || global.is_null() {
        return Err(JsValue::from_str("the editor bundle has not loaded"));
    }

    global.dyn_into()
}

/// Look up a method on an object and call it.
///
/// One helper rather than five, because the failure is always the same shape
/// and always ends the same way: the caller gets an `Err`, nothing is unwrapped
/// and the page stays alive.
fn call(target: &JsValue, method: &str, args: &[&JsValue]) -> Result<JsValue, JsValue> {
    let function: Function = Reflect::get(target, &JsValue::from_str(method))?.dyn_into()?;

    match args {
        [] => function.call0(target),
        [a] => function.call1(target, a),
        [a, b] => function.call2(target, a, b),
        _ => Err(JsValue::from_str("unsupported arity")),
    }
}

/// One mounted editor.
///
/// Holds the JavaScript handle and the closure the bundle calls back through.
/// Both die together in [`Editor::destroy`] - a closure dropped while the
/// editor can still fire it is a call into freed memory, which in wasm is not a
/// segfault but silent nonsense.
pub struct Editor {
    handle: JsValue,
    /// Kept alive for exactly as long as the editor is. `Option` so that
    /// `destroy` can take it and drop it in the right order.
    on_change: Option<Closure<dyn FnMut(String, String)>>,
}

impl Editor {
    /// Put an editor in `host`, reporting every change through `on_change`.
    ///
    /// `host` must be a node no framework writes to after this returns.
    pub fn mount(
        host: &Element,
        content: &str,
        label: Option<&str>,
        editable: bool,
        mut on_change: impl FnMut(String, EditorState) + 'static,
    ) -> Result<Self, JsValue> {
        // The state arrives as JSON rather than an object - see the bundle for
        // why - so a malformed snapshot is a parse error here and not a missing
        // field somewhere further down. `unwrap_or_default` because a toolbar
        // drawn with everything switched off is a far better answer than a
        // panic, and the text itself is unaffected either way.
        let closure = Closure::new(move |html: String, state: String| {
            on_change(html, serde_json::from_str(&state).unwrap_or_default());
        });

        let options = Object::new();
        set(&options, "content", &JsValue::from_str(content))?;
        set(&options, "editable", &JsValue::from_bool(editable))?;
        if let Some(label) = label {
            set(&options, "label", &JsValue::from_str(label))?;
        }
        set(&options, "onChange", closure.as_ref())?;

        let global = global()?;
        let handle = call(&global, "mount", &[host.as_ref(), options.as_ref()])?;

        Ok(Self {
            handle,
            on_change: Some(closure),
        })
    }

    /// Run one of the bundle's named commands.
    ///
    /// The name comes from [`Command`](super::Command), which is a closed enum,
    /// so a name the bundle does not know is a compile error rather than a
    /// button that silently does nothing.
    pub fn command(&self, name: &str, argument: Option<&str>) {
        let argument = argument.map_or(JsValue::UNDEFINED, JsValue::from_str);

        if let Err(err) = call(
            &self.handle,
            "command",
            &[&JsValue::from_str(name), &argument],
        ) {
            web_sys::console::error_1(&err);
        }
    }

    /// Replace the document, without reporting the replacement back.
    pub fn set_content(&self, html: &str) {
        if let Err(err) = call(&self.handle, "setContent", &[&JsValue::from_str(html)]) {
            web_sys::console::error_1(&err);
        }
    }

    pub fn set_editable(&self, editable: bool) {
        if let Err(err) = call(
            &self.handle,
            "setEditable",
            &[&JsValue::from_bool(editable)],
        ) {
            web_sys::console::error_1(&err);
        }
    }

    /// Tear the editor down and release the callback.
    ///
    /// Consuming, so that the closure cannot outlive the editor that calls it:
    /// there is no way to spell "destroyed, but I kept the handle".
    pub fn destroy(mut self) {
        if let Err(err) = call(&self.handle, "destroy", &[]) {
            web_sys::console::error_1(&err);
        }

        // After `destroy`, so that a teardown that fires one last transaction
        // still has somewhere to fire it.
        drop(self.on_change.take());
    }
}

fn set(target: &Object, key: &str, value: &JsValue) -> Result<(), JsValue> {
    Reflect::set(target, &JsValue::from_str(key), value).map(|_| ())
}

// ---------------------------------------------------------------------------
// The Leptos side: one editor, alive for as long as the component is.
// ---------------------------------------------------------------------------

/// What the toolbar talks to.
///
/// `Copy`, because every button in the toolbar takes one and a `Clone` would
/// have to be made per button per render. The editor itself lives in a
/// `StoredValue`, which is an arena index rather than the value.
#[derive(Clone, Copy)]
pub struct Dispatch {
    /// `LocalStorage`, because an [`Editor`] holds a `Closure` and a `Closure`
    /// is neither `Send` nor `Sync`. Leptos's default arena wants both so that
    /// a value can be read from a server thread; a value that only exists in a
    /// browser has one thread and does not need the guarantee.
    editor: StoredValue<Option<Editor>, LocalStorage>,
}

impl Dispatch {
    /// Mount an editor into `host` once it appears, and keep it in step.
    ///
    /// Returns immediately: the bundle is fetched in the background, and the
    /// toolbar stays inert until `ready` says otherwise.
    pub fn install(
        host: NodeRef<leptos::html::Div>,
        value: RwSignal<String>,
        state: RwSignal<EditorState>,
        ready: RwSignal<bool>,
        disabled: Signal<bool>,
        label: Option<String>,
    ) -> Self {
        let editor: StoredValue<Option<Editor>, LocalStorage> = StoredValue::new_local(None);

        // What the editor last told us it holds. The guard that stops the two
        // signals chasing each other: without it, writing the editor's own
        // HTML into `value` fires the effect below, which writes it back into
        // the editor, which reports a change, which writes it into `value`.
        let mirror = StoredValue::new(String::new());

        // `NodeRef` resolves after the element is in the document, so this runs
        // once with `None` and again with the div.
        Effect::new(move |mounted: Option<bool>| {
            if mounted == Some(true) {
                return true;
            }

            let Some(host) = host.get() else {
                return false;
            };

            // Cloned rather than moved: the effect is `FnMut` - it ran once
            // already, with a `NodeRef` that had not resolved yet - so nothing
            // it captures may be consumed.
            let label = label.clone();

            spawn_local(async move {
                if let Err(err) = load().await {
                    web_sys::console::error_1(&err);
                    return;
                }

                let content = value.get_untracked();
                mirror.set_value(content.clone());

                let mounted = Editor::mount(
                    &host,
                    &content,
                    label.as_deref(),
                    !disabled.get_untracked(),
                    move |html, snapshot| {
                        // `try_*` throughout: a transaction can arrive while
                        // the component is being torn down, and a reactive
                        // value read after its owner is disposed is a panic -
                        // which in wasm takes the whole page with it.
                        mirror.try_set_value(html.clone());
                        value.try_set(html);
                        state.try_set(snapshot);
                    },
                );

                match mounted {
                    Ok(mounted) => {
                        editor.try_set_value(Some(mounted));
                        ready.try_set(true);
                    }
                    Err(err) => web_sys::console::error_1(&err),
                }
            });

            true
        });

        // The form writing into the editor: a reset, a reload, a draft
        // restored. Only when the incoming value is not the one the editor
        // just gave us - see `mirror`.
        Effect::new(move |_| {
            let incoming = value.get();

            if mirror.with_value(|held| *held == incoming) {
                return;
            }

            editor.with_value(|editor| {
                if let Some(editor) = editor {
                    editor.set_content(&incoming);
                }
            });
            mirror.set_value(incoming);
        });

        // Gating a field hides nothing - see `ui::form` - so a field somebody
        // may read and not change stays on screen, holding its value, and
        // stops accepting keystrokes.
        Effect::new(move |_| {
            let disabled = disabled.get();

            editor.with_value(|editor| {
                if let Some(editor) = editor {
                    editor.set_editable(!disabled);
                }
            });
        });

        on_cleanup(move || {
            // ProseMirror registers listeners on the document and the window,
            // not only inside its own node. Dropping the Rust side without
            // this leaves them attached to an element that has been removed.
            if let Some(Some(editor)) = editor.try_update_value(Option::take) {
                editor.destroy();
            }
        });

        Self { editor }
    }

    pub fn run(self, command: Command, argument: Option<String>) {
        self.editor.with_value(|editor| {
            if let Some(editor) = editor {
                editor.command(command.name(), argument.as_deref());
            }
        });
    }
}
