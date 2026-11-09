use leptos::prelude::*;
use uuid::Uuid;

/// Deterministic from `page_id` alone, so `WikiEditor`'s own mount effect
/// and `WikiPage`'s Save handler (in app.rs) always agree on which DOM id
/// the JS island is attached to, without needing a callback prop between
/// them — `WikiPage` just calls this itself.
#[must_use]
pub fn wiki_editor_mount_id(page_id: Uuid) -> String {
    format!("wiki-editor-{page_id}")
}

/// Mounts the BlockNote React island (built separately by `editor/`, see
/// its README) into a div, loaded only on `/wiki` routes via a
/// `leptos_meta::Script`/`Stylesheet` pair in `WikiPage` — not the whole
/// app, so Cards/Decks/Review never ship React to people who don't touch
/// the wiki.
///
/// Same shared-view-tree shape as `RecordButton` in media_recorder.rs and
/// for the same reason: an earlier version of that component rendered
/// different DOM in ssr vs. hydrate and crashed hydration outright. Here
/// only the mount Effect's *behavior* is feature-gated, never the view.
// initial_document_json has to be an owned String, not &str: it's moved
// into Effect::new's 'static closure below, which outlives this function
// call — clippy can't see that requirement from the signature alone.
#[allow(clippy::must_use_candidate, clippy::needless_pass_by_value)]
#[component]
pub fn WikiEditor(
    page_id: Uuid,
    /// The page's current revision, as the stored `content_json` string —
    /// `"{}"` for a brand-new page, which the JS side treats as "start
    /// from BlockNote's own empty document" (see editor/src/main.tsx's
    /// `parseInitialContent`).
    initial_document_json: String,
    read_only: bool,
    /// Called once, right as this editor unmounts, with its final
    /// (content_json, content_markdown) — only when `!read_only`. Every
    /// path that tears this editor down without going through `WikiPage`'s
    /// own explicit "Done" save (right now: navigating to a different page
    /// mid-edit, which never calls Done at all) needs this to avoid
    /// silently dropping whatever was typed since the last autosave tick;
    /// `WikiPage`'s callback is the one that actually decides whether
    /// there's anything worth saving (comparing against its own
    /// last-saved baseline) and whether to leave edit mode — this
    /// component has no opinion on either, it just reports what it had.
    #[prop(optional, into)]
    on_unmount_snapshot: Option<Callback<(String, String)>>,
) -> impl IntoView {
    let mount_id = wiki_editor_mount_id(page_id);

    {
        let mount_id = mount_id.clone();
        let initial_document_json = initial_document_json.clone();
        Effect::new(move |_| {
            #[cfg(feature = "hydrate")]
            browser::mount(&mount_id, &initial_document_json, read_only);
            #[cfg(not(feature = "hydrate"))]
            {
                let _ = (&mount_id, &initial_document_json, read_only);
            }
        });
    }

    {
        let mount_id = mount_id.clone();
        on_cleanup(move || {
            #[cfg(feature = "hydrate")]
            {
                match (read_only, on_unmount_snapshot) {
                    (false, Some(cb)) => {
                        browser::unmount_with_snapshot(
                            &mount_id,
                            move |content_json, content_markdown| {
                                cb.run((content_json, content_markdown));
                            },
                        );
                    }
                    _ => browser::unmount(&mount_id),
                }
            }
            #[cfg(not(feature = "hydrate"))]
            {
                let _ = (&mount_id, read_only, &on_unmount_snapshot);
            }
        });
    }

    view! { <div class="wiki-editor-mount" id=mount_id></div> }
}

#[cfg(feature = "hydrate")]
mod browser {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = window, js_name = mountWikiEditor)]
        fn mount_wiki_editor(
            element_id: &str,
            initial_document_json: Option<String>,
            read_only: bool,
        );

        #[wasm_bindgen(js_namespace = window, js_name = unmountWikiEditor)]
        fn unmount_wiki_editor(element_id: &str);

        #[wasm_bindgen(js_namespace = window, js_name = getWikiEditorDocument, catch)]
        fn get_wiki_editor_document(element_id: &str) -> Result<String, JsValue>;

        #[wasm_bindgen(js_namespace = window, js_name = getWikiEditorMarkdown, catch)]
        fn get_wiki_editor_markdown(element_id: &str) -> Result<js_sys::Promise, JsValue>;
    }

    /// Injects `wiki-editor.js`/`.css` itself rather than leaning on a
    /// `leptos_meta::Script`/`Stylesheet` placed inside `WikiPage`'s
    /// dynamically-toggled edit branch — confirmed live that tags declared
    /// there never actually reach the DOM post-hydration (`leptos_meta`'s
    /// head-tag machinery appears to assume a stable position in the tree,
    /// not one that mounts for the first time after initial hydration, deep
    /// inside a `Suspend`-resolved, conditionally-rendered branch): the
    /// module itself loads and runs fine the moment it's actually requested
    /// — confirmed by mounting it manually from the console — so the
    /// missing piece was purely the `<script>` tag never being added at
    /// all, not anything about the bundle. A plain `id` check on each
    /// element keeps this idempotent across repeated `WikiEditor` mounts.
    fn ensure_editor_assets_injected() {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if document.get_element_by_id("wiki-editor-script").is_none()
            && let Ok(script) = document.create_element("script")
        {
            let _ = script.set_attribute("type", "module");
            let _ = script.set_attribute("src", "/wiki-editor.js");
            let _ = script.set_attribute("id", "wiki-editor-script");
            if let Some(head) = document.head() {
                let _ = head.append_child(&script);
            }
        }
        if document
            .get_element_by_id("wiki-editor-stylesheet")
            .is_none()
            && let Ok(link) = document.create_element("link")
        {
            let _ = link.set_attribute("rel", "stylesheet");
            let _ = link.set_attribute("href", "/wiki-editor.css");
            let _ = link.set_attribute("id", "wiki-editor-stylesheet");
            if let Some(head) = document.head() {
                let _ = head.append_child(&link);
            }
        }
    }

    /// `wiki-editor.js` is a module script, which runs asynchronously, so
    /// `window.mountWikiEditor` may not exist yet the instant this fires.
    /// Polled rather than wired through the script's own `load` event: far
    /// less glue code for a one-time, short wait, and safe to repeat since
    /// mounting is idempotent per element id.
    ///
    /// Also waits for the mount point itself, not just the script: this
    /// Effect can fire before the `<div id=element_id>` this same component
    /// just rendered has actually landed in the live DOM (the view tree
    /// resolving and the DOM patch applying aren't the same instant, deeper
    /// inside a `Suspend`-resolved, conditionally-rendered branch — this
    /// one is both). Confirmed live: `mount_wiki_editor` throwing "no
    /// element with id ..." on the very first Edit click of a fresh page,
    /// with the div demonstrably present moments later — a real,
    /// repeatable race, not a one-off. Without this, that one lost race
    /// permanently stranded the page in edit mode with no toolbar and no
    /// editor at all, since nothing here ever retried.
    pub(super) fn mount(element_id: &str, initial_document_json: &str, read_only: bool) {
        ensure_editor_assets_injected();
        let element_id = element_id.to_string();
        let initial_document_json = initial_document_json.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            if !wait_for_editor_script().await {
                leptos::logging::error!(
                    "wiki-editor.js never finished loading — editor not mounted"
                );
                return;
            }
            if !wait_for_mount_element(&element_id).await {
                leptos::logging::error!(
                    "wiki editor mount point {element_id} never appeared in the DOM — editor not mounted"
                );
                return;
            }
            mount_wiki_editor(&element_id, Some(initial_document_json), read_only);
        });
    }

    pub(super) fn unmount(element_id: &str) {
        if has_global("unmountWikiEditor") {
            unmount_wiki_editor(element_id);
        }
    }

    /// Unmounts like `unmount` above, but first captures this editor's
    /// final document and hands it to `on_snapshot`, resolved to plain
    /// strings — the one piece `WikiPage` needs to flush a pending edit on
    /// paths that don't go through its own explicit "Done" save (right
    /// now: navigating to a different page while editing).
    ///
    /// The capture itself — both `get_wiki_editor_document` and the
    /// `get_wiki_editor_markdown` call that *starts* its Promise — happens
    /// synchronously, strictly before `unmount` below deletes this
    /// element's entry in `window.__wikiEditors`. Doing this from a task
    /// spawned by some *other* effect reacting to the same navigation
    /// (tried first) doesn't work: `wasm_bindgen_futures::spawn_local`
    /// only polls its future on the next microtask, by which point this
    /// same synchronous `on_cleanup` — the one that deletes the handle —
    /// has already finished. Only *awaiting* the already-started markdown
    /// Promise is deferred to that later microtask, which is fine: once
    /// obtained, a Promise is a value in its own right, independent of
    /// whether the JS object it came from still exists.
    pub(super) fn unmount_with_snapshot(
        element_id: &str,
        on_snapshot: impl FnOnce(String, String) + 'static,
    ) {
        let snapshot = get_wiki_editor_document(element_id)
            .ok()
            .zip(get_wiki_editor_markdown(element_id).ok());
        unmount(element_id);
        if let Some((content_json, markdown_promise)) = snapshot {
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(value) = JsFuture::from(markdown_promise).await
                    && let Some(content_markdown) = value.as_string()
                {
                    on_snapshot(content_json, content_markdown);
                }
            });
        }
    }

    /// Current document, serialized as JSON — what `save_page_revision`
    /// stores as `content_json`.
    ///
    /// # Errors
    /// If no editor is mounted at `element_id`, or the JS call itself throws.
    pub async fn get_document(element_id: &str) -> Result<String, String> {
        get_wiki_editor_document(element_id).map_err(|e| format!("{e:?}"))
    }

    /// Current document exported to markdown (lossy — see the wiki
    /// planning notes on why `content_json` stays the source of truth) —
    /// what `save_page_revision` stores as `content_markdown`.
    ///
    /// # Errors
    /// If no editor is mounted at `element_id`, the JS call throws, or the
    /// export doesn't come back as a string.
    pub async fn get_markdown(element_id: &str) -> Result<String, String> {
        let promise = get_wiki_editor_markdown(element_id).map_err(|e| format!("{e:?}"))?;
        let value = JsFuture::from(promise)
            .await
            .map_err(|e| format!("{e:?}"))?;
        value
            .as_string()
            .ok_or_else(|| "markdown export wasn't a string".to_string())
    }

    /// 30s, not the 5s `wait_for_mount_element` below gets away with: unlike
    /// that one (which just waits on DOM Leptos already rendered), this
    /// waits on a ~1.6MB network fetch of `wiki-editor.js` — on a cold Fly
    /// machine (`min_machines_running = 0` in `fly.toml`), the whole app is
    /// still warming up and 5s wasn't always enough, stranding the editor
    /// mount point blank with nothing in the console but this function's
    /// own timeout error.
    async fn wait_for_editor_script() -> bool {
        for _ in 0..600 {
            if has_global("mountWikiEditor") {
                return true;
            }
            sleep(50).await;
        }
        false
    }

    async fn wait_for_mount_element(element_id: &str) -> bool {
        for _ in 0..100 {
            if has_element(element_id) {
                return true;
            }
            sleep(50).await;
        }
        false
    }

    fn has_global(name: &str) -> bool {
        web_sys::window()
            .and_then(|w| js_sys::Reflect::has(&w, &JsValue::from_str(name)).ok())
            .unwrap_or(false)
    }

    fn has_element(id: &str) -> bool {
        web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(id))
            .is_some()
    }

    async fn sleep(millis: i32) {
        sleep_ms(millis).await;
    }

    /// Also used directly by `WikiPage`'s autosave loop (app.rs) — not just
    /// internal to this module's own polling.
    ///
    /// # Panics
    /// If called outside a browser context (`web_sys::window()` returns
    /// `None`) — never true in practice, this only ever runs client-side.
    pub async fn sleep_ms(millis: i32) {
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            let window = web_sys::window().expect("window");
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, millis);
        });
        let _ = JsFuture::from(promise).await;
    }
}

#[cfg(feature = "hydrate")]
pub use browser::{get_document, get_markdown, sleep_ms};
