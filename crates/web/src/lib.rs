// The #[server] macro's generated client-side (hydrate) stub trips this —
// nothing in code we wrote.
#![allow(clippy::unused_async_trait_impl)]
// WikiPage's view (app.rs) nests Suspense/Transition/match/closures deep
// enough that rustc's type-level query recursion for AnyView::into_any's
// hydrate_async overflows the default 128 limit in release builds —
// confirmed failing a `cargo leptos build --release` with "queries
// overflow the depth limit!" otherwise. Debug builds don't hit it, which
// is why this only ever showed up in a release/Docker build, not CI's
// debug build or clippy.
#![recursion_limit = "512"]

pub mod account;
pub mod app;
pub mod auth;
pub mod cards;
pub mod db;
pub mod decks;
pub mod dictionary;
pub mod dictionary_ui;
pub mod email;
pub mod image_search;
pub mod media_recorder;
pub mod state;
pub mod storage;
pub mod wiki;
pub mod wiki_editor;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(app::App);

    // Hydration — and so the client-side handlers server-action forms
    // depend on — isn't actually done until this point. The WASM bundle's
    // own network fetch completing is not a usable proxy for that: e2e
    // traces show a server-action form submit still landing as a plain
    // (unhydrated) native POST under 200ms after that fetch resolves,
    // well short of hydrate_body returning. This attribute is the only
    // reliable signal, so e2e tests wait on it instead.
    if let Some(body) = leptos::prelude::document().body() {
        let _ = body.set_attribute("data-hydrated", "true");
    }
}
