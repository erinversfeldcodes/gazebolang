// #[component] already enforces call sites use the returned view; clippy's
// pedantic must_use_candidate doesn't see through the macro expansion.
#![allow(clippy::must_use_candidate)]

use crate::account::{
    ChangePassword, ConfirmPasswordReset, CreateInvite, LogIn, RequestPasswordReset,
    SetDailyNewCardLimit, SignUpViaInvite, UpdateEmail, UpdateName, current_profile,
    current_user_name, user_stats,
};
use crate::cards::{
    AddCardLink, CARD_TYPES, CARDS_PAGE_SIZE, CardLink, DeleteCard, DirectionInput, GradeCard,
    GradeInput, ImageSearchResult, ImportCardImage, LINK_TYPES, RemoveCardLink, ReviewOrderInput,
    SearchCardImages, get_card, list_tags, review_queue, search_cards,
};
use crate::decks::{
    AddCardToDeck, CreateDeck, Deck, DeleteDeck, RemoveCardFromDeck, RenameDeck, list_decks,
};
use crate::dictionary::{WordDefinition, get_word_definition};
use crate::dictionary_ui::{ClickableWord, DictionaryDetailsEditor};
use crate::media_recorder::RecordButton;
use crate::trainer_back_chain::BackChainGamePage;
use crate::trainer_minimal_pairs::MinimalPairTrainerPage;
use crate::trainer_sound_spelling::SoundSpellingTrainerPage;
use crate::trainer_spelling::SpellingTestPage;
use crate::wiki::{
    AddPageToCard, CreatePage, DeletePage, MovePage, PageRevisionSummary, PageTreeNode,
    RemovePageFromCard, RenamePage, ReorderPage, RestoreRevision, RevisionDiff, SavePageRevision,
    cards_for_page, get_page, get_revision_diff, list_page_revisions, list_page_tree, search_pages,
};
// Only called from the Done handler's #[cfg(feature = "hydrate")] branch
// (see WikiPage below) — an unused-import warning (and so a clippy -D
// warnings failure) under the ssr-only build otherwise.
#[cfg(feature = "hydrate")]
use crate::wiki::save_page_revision;
use crate::wiki_editor::WikiEditor;
use leptos::html;
use leptos::prelude::*;
// Deliberately not leptos_meta::{Stylesheet, Title, MetaTags,
// provide_meta_context} — see shell()'s doc comment for why.
use leptos_router::{
    StaticSegment,
    components::{A, Form, Redirect, Route, Router, Routes},
    hooks::{use_location, use_navigate, use_params_map, use_query_map},
    path,
};
use uuid::Uuid;

/// `card.youtube_url` is always already a normalized
/// `youtube.com/embed/{id}` URL by the time it reaches here (validated
/// and canonicalized server-side in main.rs, never the raw link a user
/// pasted), so this is just the embed markup — no parsing needed. Shared
/// across the card list, card detail, and review pages rather than
/// repeated, since it's the exact same three attributes everywhere.
pub(crate) fn youtube_embed(url: String) -> impl IntoView {
    view! { <iframe src=url width="560" height="315" allowfullscreen=true></iframe> }
}

/// Renders a `?error=` query param — left by main.rs's raw multipart POST
/// handlers (`cards_new`/`cards_bulk`/`cards_update`) on a failed
/// submission, see `error_redirect` there — with the same `"Error: "`
/// paragraph every `ActionForm`-based error in this file already uses. A
/// reactive read (every call site embeds this as `{move || form_error_banner()}`):
/// `leptos_router::Form`'s same-origin redirect handling navigates
/// client-side rather than hard-reloading, so a `?error=` that lands after
/// a `Form` submit is a query change on an already-mounted route, not a
/// fresh mount — a `get_untracked()` read here would never see it.
fn form_error_banner() -> impl IntoView {
    let query = use_query_map();
    query
        .get()
        .get("error")
        .map(|message| view! { <p>"Error: " {message}</p> })
}

/// A submit button for a destructive `ActionForm` that requires two clicks:
/// the first arms it (relabeling to "Confirm delete?" for a few seconds)
/// without submitting, the second lets the click proceed as a normal submit.
/// Avoids pulling in a modal/dialog for a single low-frequency action that's
/// already behind its own form; `armed` must be a signal local to this one
/// button (a fresh `RwSignal::new(false)` per call site, or per loop
/// iteration for a repeated button like one delete-per-row).
fn confirm_button(label: &'static str, armed: RwSignal<bool>) -> impl IntoView {
    view! {
        <button
            type="submit"
            class:armed=armed
            on:click=move |ev| {
                if !armed.get_untracked() {
                    ev.prevent_default();
                    armed.set(true);
                    set_timeout(move || armed.set(false), std::time::Duration::from_secs(4));
                }
            }
        >
            {move || if armed.get() { "Confirm delete?" } else { label }}
        </button>
    }
}

/// Search-and-pick an image for a card's `image_url`, as an alternative to
/// uploading a file directly. The English text here is deliberately never
/// part of the submitted card form — it only ever drives
/// `search_card_images`, never gets stored or shown on the card itself
/// (unlike target_text, which is the target-language word/phrase).
/// Picking a thumbnail re-fetches and re-hosts the full image server-side
/// (see `cards::import_card_image`) rather than linking the provider's own
/// URL, so a card's image is always something this app hosts, same as a
/// direct upload. The caller renders the resulting `chosen_image_url` as a
/// hidden form field — see `NewCardPage`/`EditCardPage`.
#[component]
fn ImageSearchPicker(
    chosen_image_url: RwSignal<Option<String>>,
    /// Pre-fills the search field — used by `BulkAddPage`, where the
    /// English word was already given as part of the pasted list, so the
    /// user shouldn't have to retype it per card just to search.
    #[prop(default = String::new())]
    initial_query: String,
) -> impl IntoView {
    let english_text = RwSignal::new(initial_query.clone());
    let search = ServerAction::<SearchCardImages>::new();
    let import = ServerAction::<ImportCardImage>::new();

    Effect::new(move |_| {
        if let Some(Ok(url)) = import.value().get() {
            chosen_image_url.set(Some(url));
        }
    });

    view! {
        <div class="image-search">
            <label>
                "English translation (used only to search for an image — never stored on the card)"
                <input
                    type="text"
                    value=initial_query
                    on:input=move |ev| english_text.set(event_target_value(&ev))
                />
            </label>
            <button
                type="button"
                on:click=move |_| {
                    search.dispatch(SearchCardImages { query: english_text.get_untracked() });
                }
            >
                "Search images"
            </button>
            {move || search.pending().get().then(|| view! { <p>"Searching…"</p> })}
            // Once an image is chosen, the stale results grid from whatever
            // search produced it has nothing left to do — confirmed live it
            // was staying mounted (search.value() only ever changes on a
            // new search, not on a pick) and sitting there underneath the
            // chosen-image preview. Hidden rather than cleared: "Remove"
            // below un-chooses without forcing a re-search, and the same
            // results reappear right where they were.
            {move || {
                (chosen_image_url.get().is_none())
                    .then(|| {
                        view! {
                            {move || {
                                // Distinguishes "haven't searched yet" (None, show
                                // nothing) from "searched and every provider came
                                // back empty" (an Ok with no results) — without
                                // this, a search that genuinely found nothing
                                // looks identical to never having searched at
                                // all, with no signal that redoing it with
                                // different text might help.
                                search
                                    .value()
                                    .get()
                                    .and_then(|result| match result {
                                        Ok(results) if results.is_empty() => {
                                            Some(
                                                view! {
                                                    <p>"No images found — try different search text."</p>
                                                }
                                                    .into_any(),
                                            )
                                        }
                                        Ok(_) => None,
                                        Err(e) => {
                                            Some(view! { <p>"Error: " {e.to_string()}</p> }.into_any())
                                        }
                                    })
                            }}
                            <div class="image-search-results">
                                {move || {
                                    let results: Vec<ImageSearchResult> = search
                                        .value()
                                        .get()
                                        .and_then(Result::ok)
                                        .unwrap_or_default();
                                    results
                                        .into_iter()
                                        .map(|r| {
                                            let provider = r.provider.clone();
                                            let id = r.id.clone();
                                            view! {
                                                <img
                                                    src=r.thumbnail_url
                                                    alt=r.description
                                                    class="image-search-thumb"
                                                    data-provider=r.provider.clone()
                                                    on:click=move |_| {
                                                        import
                                                            .dispatch(ImportCardImage {
                                                                provider: provider.clone(),
                                                                id: id.clone(),
                                                            });
                                                    }
                                                />
                                            }
                                        })
                                        .collect_view()
                                }}
                            </div>
                        }
                    })
            }}
            {move || import.pending().get().then(|| view! { <p>"Adding image…"</p> })}
            {move || {
                import.value().get().and_then(Result::err).map(|e| {
                    view! { <p>"Error: " {e.to_string()}</p> }
                })
            }}
            {move || {
                chosen_image_url.get().map(|url| view! {
                    <div class="image-search-chosen">
                        <img src=url alt="chosen image" />
                        <button type="button" on:click=move |_| chosen_image_url.set(None)>
                            "Remove"
                        </button>
                    </div>
                })
            }}
        </div>
    }
}

/// Which of `MediaAttachmentFields`' image/audio fields a given card type
/// actually uses — `image_to_target`/`sound_to_written`/`image_to_audio`
/// each want a specific, narrow subset. Video and `YouTube` aren't part of
/// this: they're supplementary media that can ride alongside *any* type
/// (`ReviewPage`'s generic `media_view` already renders whichever of the
/// four fields a card has, for every type except `image_to_audio`/
/// `sentence_translation`), not something tied to one particular type the
/// way image and audio are — confirmed live as a real regression, not
/// just a test-script mismatch, when gating them the same way made
/// video/`YouTube` unreachable from every card type's own create/edit
/// form.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MediaFields {
    Image,
    Audio,
    ImageAndAudio,
}

/// `card_type`'s nominal `MediaFields` — shared by `NewCardPage`/
/// `BulkAddPage` (a brand new card only ever has the image/audio fields
/// its chosen type actually uses, though video/YouTube are always
/// offered regardless — see `MediaFields`'s own doc comment). `EditCardPage`
/// starts from this too, but widens it to `ImageAndAudio` when the card
/// already has the *other* one set — see its own call site.
fn media_fields_for(card_type: &str) -> MediaFields {
    match card_type {
        "sound_to_written" => MediaFields::Audio,
        "image_to_target" => MediaFields::Image,
        _ => MediaFields::ImageAndAudio,
    }
}

/// The image/audio/video/YouTube attachment fields shared by `NewCardPage`,
/// `EditCardPage`, and `BulkAddPage` — previously hand-duplicated between
/// the first two with only label wording and the presence of "remove"
/// checkboxes differing. `editing` switches between "Image file" (create)
/// and "Replace image (optional)" (edit) wording for all four fields; the
/// `has_*` flags gate each "Remove X" checkbox, since there's nothing to
/// remove on a brand new card (NewCardPage/BulkAddPage never pass `true`
/// for any of them).
#[component]
fn MediaAttachmentFields(
    chosen_image_url: RwSignal<Option<String>>,
    audio_input: NodeRef<html::Input>,
    video_input: NodeRef<html::Input>,
    #[prop(default = false)] editing: bool,
    #[prop(default = false)] has_image: bool,
    #[prop(default = false)] has_audio: bool,
    #[prop(default = false)] has_video: bool,
    #[prop(default = false)] has_youtube: bool,
    #[prop(default = MediaFields::ImageAndAudio)] fields: MediaFields,
    // Appended to every field name below ("image", "audio", "card_type",
    // ...) — empty for NewCardPage/EditCardPage (one card per form, so
    // plain names), `"_{idx}"` for BulkAddPage (many cards in one
    // multipart submission, needing distinguishable field names per draft
    // — see main.rs's parse_bulk_multipart).
    #[prop(default = String::new())] field_suffix: String,
    // Pre-fills ImageSearchPicker's own search field — see its
    // `initial_query` doc comment for why (BulkAddPage already knows the
    // English word from the pasted list; NewCardPage/EditCardPage have
    // nothing to prefill it with).
    #[prop(default = String::new())] search_query: String,
) -> impl IntoView {
    let image_label = if editing {
        "Replace image (optional)"
    } else {
        "Image file"
    };
    let audio_label = if editing {
        "Replace audio (optional)"
    } else {
        "Audio file"
    };
    let video_label = if editing {
        "Replace video (optional)"
    } else {
        "Video file"
    };
    let youtube_label = if editing {
        "Replace YouTube link (optional)"
    } else {
        "Or link a YouTube video"
    };

    let audio_fields = view! {
        <label>
            {audio_label}
            <input
                type="file"
                name=format!("audio{field_suffix}")
                accept="audio/*"
                node_ref=audio_input
            />
        </label>
        <RecordButton kind="audio" target_input=audio_input />
        {has_audio.then(|| view! {
            <label>
                <input type="checkbox" name=format!("remove_audio{field_suffix}") />
                "Remove audio"
            </label>
        })}
    };

    let image_fields = view! {
        <label>
            {image_label}
            <input type="file" name=format!("image{field_suffix}") accept="image/*" />
        </label>
        <ImageSearchPicker chosen_image_url=chosen_image_url initial_query=search_query />
        {
            let field_suffix = field_suffix.clone();
            move || {
                let field_suffix = field_suffix.clone();
                chosen_image_url.get().map(|url| view! {
                    <input type="hidden" name=format!("image_url{field_suffix}") value=url />
                })
            }
        }
        {has_image.then(|| view! {
            <label>
                <input type="checkbox" name=format!("remove_image{field_suffix}") />
                "Remove image"
            </label>
        })}
    };

    let image_or_audio_fields = match fields {
        MediaFields::Image => image_fields.into_any(),
        MediaFields::Audio => audio_fields.into_any(),
        MediaFields::ImageAndAudio => view! { {image_fields} {audio_fields} }.into_any(),
    };

    view! {
        {image_or_audio_fields}
        <label>
            {video_label}
            <input
                type="file"
                name=format!("video{field_suffix}")
                accept="video/*"
                node_ref=video_input
            />
        </label>
        <RecordButton kind="video" target_input=video_input />
        {has_video.then(|| view! {
            <label>
                <input type="checkbox" name=format!("remove_video{field_suffix}") />
                "Remove video"
            </label>
        })}
        <label>
            {youtube_label}
            <input
                type="url"
                name=format!("youtube_url{field_suffix}")
                placeholder="https://www.youtube.com/watch?v=..."
            />
        </label>
        {has_youtube.then(|| view! {
            <label>
                <input type="checkbox" name=format!("remove_youtube{field_suffix}") />
                "Remove YouTube link"
            </label>
        })}
    }
    .into_any()
}

// `<Stylesheet>`/`<Title>`/`<MetaTags/>` (leptos_meta) are deliberately not
// used here, even though both tags are static and sit at an unconditional,
// top-level position in App()'s view — confirmed live (gazebo.fly.dev
// panicking on every /cards/:id hydration, "the framework expected a marker
// node, but found this instead: nav") that leptos_meta 0.8.7 (on leptos
// 0.8.22/tachys 0.2.19, the latest mutually-released 0.8.x line as of this
// fix) doesn't leave a matching hydration marker in <body> for tags whose
// actual rendered output is teleported into <head> — the client-side
// hydration walker then consumes the Router's own first node (<nav>) trying
// to satisfy that phantom expectation, and the whole page's event listeners
// never finish attaching. WikiEditor already hit the same class of bug for
// its own injected <script>/<link> and worked around it by never going
// through leptos_meta at all (see its own doc comment) — this is that same
// fix applied to the two tags App() itself declared. Both are static (never
// change per route), so hardcoding them directly into this non-hydrated
// shell is strictly equivalent and sidesteps the mechanism entirely.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <AutoReload options=options.clone() />
                <HydrationScripts options/>
                <link id="leptos" rel="stylesheet" href="/pkg/web.css"/>
                <title>"Gazebo"</title>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    let auth = Resource::new(|| (), |()| current_user_name());

    view! {
        <Router>
            <nav>
                // Grouped by purpose rather than 9 flat links: content you
                // browse stays flat and always visible (including signed
                // out, since all three are public-read); creating a card
                // and account/admin actions each collapse into their own
                // <details> menu, the same disclosure pattern used
                // everywhere else in this app (Reveal, dictionary details)
                // rather than a new interaction model. Also makes room for
                // Grammar without the nav growing past this.
                <Transition fallback=|| view! { <A href="/" exact=true>"Cards"</A> }>
                    {move || {
                        // Read reactively *before* the Suspend's async block
                        // — a signal read after an .await point inside
                        // Suspend doesn't reliably retrigger this closure,
                        // so the current-page highlight would go stale.
                        let path = use_location().pathname.get();
                        let in_cards_section =
                            path == "/" || path == "/new" || path == "/bulk-add"
                                || path.starts_with("/cards/");
                        Suspend::new(async move {
                            // Signed out: a plain link, same as Decks/Grammar
                            // — there's nothing to add without an account.
                            // Signed in: folded into one dropdown rather
                            // than a separate top-level "Add" — card
                            // creation is the only thing it ever did
                            // (Decks/Grammar already have their own inline
                            // "create" UI), so a second nav item for it was
                            // redundant. The trigger is a real <A>, opened on
                            // hover (CSS :hover/:focus-within on .nav-menu in
                            // main.css) rather than click, so clicking it
                            // navigates straight to "/" instead of toggling
                            // a menu — which is also why "All cards" isn't
                            // repeated as a dropdown item below. <A exact>
                            // only covers the exact "/" match, so the broader
                            // in-section highlight still needs its own class.
                            match auth.await.ok().flatten() {
                                Some(_) => view! {
                                    <div class="nav-menu">
                                        <A href="/" exact=true {..} class:nav-menu-current=in_cards_section>
                                            "Cards"
                                        </A>
                                        <div class="nav-menu-items">
                                            <A href="/new">"Add a card"</A>
                                            <A href="/bulk-add">"Bulk add"</A>
                                        </div>
                                    </div>
                                }.into_any(),
                                None => view! { <A href="/" exact=true>"Cards"</A> }.into_any(),
                            }
                        })
                    }}
                </Transition>
                " · "
                <A href="/decks">"Decks"</A>
                " · "
                <A href="/wiki">"Grammar"</A>
                // Public like Decks/Grammar — nothing is mutated or
                // persisted by walking a back-chain, so it needs no
                // account, unlike the other two practice modes below.
                " · "
                <A href="/train/back-chain">"Back-chain"</A>
                <Transition fallback=|| ()>
                    {move || {
                        Suspend::new(async move {
                        match auth.await.ok().flatten() {
                            Some(_) => view! {
                                " · "
                                // Review and the other drills used to be two
                                // separate top-level items ("Review" plus a
                                // "Practice" dropdown); folded into one since
                                // they're all study activities and a user
                                // reaching for one is usually open to the
                                // others. Opens on hover (see .nav-menu in
                                // main.css), same as the Cards/Account
                                // menus — minimal pairs and the spelling test
                                // both need an account (their attempts are
                                // per-user), so this dropdown lives in the
                                // signed-in-only branch, unlike Back-chain
                                // above, which stays a separate, public,
                                // standalone link rather than joining this
                                // menu, since folding it in here would hide
                                // it from signed-out visitors.
                                <div class="nav-menu">
                                    <A href="/review" exact=true>"Review"</A>
                                    <div class="nav-menu-items">
                                        <A href="/train/minimal-pairs">"Minimal pairs"</A>
                                        <A href="/train/spelling">"Spelling test"</A>
                                        <A href="/train/sound-spelling">"Sound-spelling"</A>
                                    </div>
                                </div>
                                " · "
                                // Opens on hover (see .nav-menu in main.css),
                                // same as the Cards menu above — clicking
                                // "Account" navigates straight to /account,
                                // so it isn't repeated as a dropdown item.
                                <div class="nav-menu">
                                    <A href="/account" exact=true>"Account"</A>
                                    <div class="nav-menu-items">
                                        <A href="/stats">"Stats"</A>
                                        <A href="/settings">"Settings"</A>
                                        <a href="/auth/logout">"Sign out"</a>
                                    </div>
                                </div>
                            }.into_any(),
                            None => view! {
                                " · "<A href="/login">"Sign in"</A>
                                " · "<A href="/signup">"Sign up"</A>
                            }.into_any(),
                        }
                        })
                    }}
                </Transition>
            </nav>
            <main>
                // Three deliberately different auth-gating shapes below,
                // not an accident of implementation order:
                //  1. No meaningful signed-out use (NewCardPage,
                //     BulkAddPage, ReviewPage, StatsPage, SettingsPage,
                //     AccountPage) hard-redirects to /login via a
                //     `<Redirect>` inside its own Suspend block.
                //  2. A genuine public read-only view (EditCardPage,
                //     DecksPage, WikiListPage, WikiPage — browsing
                //     cards/decks/wiki pages is intentionally open to
                //     anyone, per the shared-pool model) instead degrades
                //     gracefully: it renders for everyone, and conditions
                //     only the mutating bits on `signed_in.then(...)`.
                //  3. CardListPage checks nothing at all — its
                //     `<details>`/"Reveal" is a content-reveal idiom
                //     (hide the answer, not the page), unrelated to
                //     sign-in state.
                <Routes fallback=|| view! { <p>"Page not found."</p> }>
                    <Route path=StaticSegment("") view=CardListPage/>
                    <Route path=StaticSegment("new") view=NewCardPage/>
                    <Route path=StaticSegment("bulk-add") view=BulkAddPage/>
                    <Route path=path!("/cards/:id") view=EditCardPage/>
                    <Route path=StaticSegment("decks") view=DecksPage/>
                    <Route path=StaticSegment("wiki") view=WikiListPage/>
                    // The slug is cosmetic — routing is by :id alone, so a
                    // page surviving a rename/reparent never breaks a link
                    // to it, the same reason /cards/:id doesn't carry a
                    // slug either. *path is unused (not even read via
                    // use_params_map) on purpose: it only exists so the
                    // wildcard segment is present for links built as
                    // /wiki/{id}/{slug} to resolve at all.
                    <Route path=path!("/wiki/:id/*path") view=WikiPage/>
                    <Route path=StaticSegment("review") view=ReviewPage/>
                    // path!, not StaticSegment: StaticSegment matches
                    // exactly one path segment and 404s on any `/` inside
                    // its literal that the real URL hasn't already
                    // consumed — confirmed live — so a single StaticSegment
                    // can never match a multi-segment literal like
                    // "train/minimal-pairs". Same reason /cards/:id and
                    // /wiki/:id/*path above use path! instead.
                    <Route path=path!("/train/minimal-pairs") view=MinimalPairTrainerPage/>
                    <Route path=path!("/train/back-chain") view=BackChainGamePage/>
                    <Route path=path!("/train/spelling") view=SpellingTestPage/>
                    <Route path=path!("/train/sound-spelling") view=SoundSpellingTrainerPage/>
                    <Route path=StaticSegment("stats") view=StatsPage/>
                    <Route path=StaticSegment("settings") view=SettingsPage/>
                    <Route path=StaticSegment("account") view=AccountPage/>
                    <Route path=StaticSegment("login") view=LoginPage/>
                    <Route path=StaticSegment("signup") view=SignupPage/>
                    <Route path=StaticSegment("forgot-password") view=ForgotPasswordPage/>
                    <Route path=StaticSegment("reset-password") view=ResetPasswordPage/>
                </Routes>
            </main>
        </Router>
    }
}

/// Signup/login both end by setting a session cookie server-side and then
/// need the browser to actually pick it up. A client-side route change
/// (`use_navigate`, as `EditCardPage` uses after a delete) wouldn't do
/// that: `App`'s top-level nav `Resource` was already fetched once at
/// mount and a SPA-style transition never re-runs it. A full top-level
/// navigation does, so this deliberately uses a real browser navigation
/// rather than the router.
fn reload_to(path: &str) {
    _ = window().location().set_href(path);
}

#[component]
fn LoginPage() -> impl IntoView {
    let log_in = ServerAction::<LogIn>::new();
    let query = use_query_map();
    let just_reset = move || query.read().get("reset").as_deref() == Some("success");

    Effect::new(move |_| {
        if matches!(log_in.value().get(), Some(Ok(()))) {
            reload_to("/");
        }
    });

    view! {
        <h1>"Sign in"</h1>
        {move || just_reset().then(|| view! {
            <p>"Password updated — sign in with your new password."</p>
        })}
        <ActionForm action=log_in>
            <label>"Email" <input type="email" name="email" /></label>
            <label>"Password" <input type="password" name="password" /></label>
            <button type="submit">"Sign in"</button>
        </ActionForm>
        {move || {
            log_in.value().get().and_then(Result::err).map(|e| {
                view! { <p>"Error: " {e.to_string()}</p> }
            })
        }}
        <p><A href="/forgot-password">"Forgot your password?"</A></p>
    }
}

/// Reachable only via an invite link (`?token=...`) — open self-serve
/// signup doesn't exist any more, see `account::sign_up_via_invite`'s doc
/// comment. Same `use_query_map()`-seeded-token shape as
/// `ResetPasswordPage`.
#[component]
fn SignupPage() -> impl IntoView {
    let sign_up = ServerAction::<SignUpViaInvite>::new();
    let query = use_query_map();
    let token = move || query.read().get("token").unwrap_or_default();

    Effect::new(move |_| {
        if matches!(sign_up.value().get(), Some(Ok(()))) {
            reload_to("/");
        }
    });

    view! {
        <h1>"Sign up"</h1>
        {move || {
            token().is_empty().then(|| view! {
                <p>"This signup link is invalid — ask whoever invited you for a fresh one."</p>
            })
        }}
        <ActionForm action=sign_up>
            <input type="hidden" name="token" value=token() />
            <label>"Name" <input type="text" name="name" /></label>
            <label>"Password" <input type="password" name="password" /></label>
            <label>"Confirm password" <input type="password" name="confirm_password" /></label>
            <button type="submit">"Sign up"</button>
        </ActionForm>
        {move || {
            sign_up.value().get().and_then(Result::err).map(|e| {
                view! { <p>"Error: " {e.to_string()}</p> }
            })
        }}
        <p><A href="/login">"Already have an account? Sign in"</A></p>
    }
}

#[component]
fn ForgotPasswordPage() -> impl IntoView {
    let request_reset = ServerAction::<RequestPasswordReset>::new();

    view! {
        <h1>"Forgot your password?"</h1>
        <ActionForm action=request_reset>
            <label>"Email" <input type="email" name="email" /></label>
            <button type="submit">"Send reset link"</button>
        </ActionForm>
        {move || {
            request_reset
                .value()
                .get()
                .map(|result| match result {
                    Ok(()) => view! {
                        <p>"If an account with that email exists, we've sent a password reset link."</p>
                    }.into_any(),
                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                })
        }}
    }
}

#[component]
fn ResetPasswordPage() -> impl IntoView {
    let confirm_reset = ServerAction::<ConfirmPasswordReset>::new();
    let query = use_query_map();
    let token = move || query.read().get("token").unwrap_or_default();

    Effect::new(move |_| {
        if matches!(confirm_reset.value().get(), Some(Ok(()))) {
            reload_to("/login?reset=success");
        }
    });

    view! {
        <h1>"Reset your password"</h1>
        {move || {
            token().is_empty().then(|| view! { <p>"This password reset link is invalid."</p> })
        }}
        <ActionForm action=confirm_reset>
            <input type="hidden" name="token" value=token() />
            <label>"New password" <input type="password" name="password" /></label>
            <label>"Confirm new password" <input type="password" name="confirm_password" /></label>
            <button type="submit">"Reset password"</button>
        </ActionForm>
        {move || {
            confirm_reset.value().get().and_then(Result::err).map(|e| {
                view! { <p>"Error: " {e.to_string()}</p> }
            })
        }}
    }
}

#[component]
fn CardListPage() -> impl IntoView {
    // Seeded from `?deck=<id>` so DecksPage's "go see this deck's cards"
    // links actually land pre-filtered, not just on an unfiltered list.
    let query = use_query_map();
    let tag_filter = RwSignal::new(None::<String>);
    let deck_filter = RwSignal::new(
        query
            .get_untracked()
            .get("deck")
            .and_then(|id| Uuid::parse_str(&id).ok()),
    );
    let page = RwSignal::new(0_i64);

    let tags = Resource::new(|| (), |()| list_tags());
    let decks = Resource::new(|| (), |()| list_decks());
    let delete = ServerAction::<DeleteCard>::new();
    let cards = Resource::new(
        move || {
            (
                tag_filter.get(),
                deck_filter.get(),
                page.get(),
                delete.version().get(),
            )
        },
        |(tag, deck_id, page, _)| crate::cards::list_cards(tag, deck_id, page),
    );

    // A delete can empty out the last page (e.g. the one card left on page
    // 3 just got deleted) — step back a page rather than show a "no cards"
    // message the user could only escape by editing the URL or re-opening
    // a filter dropdown.
    Effect::new(move |_| {
        if let Some(Ok(result)) = cards.get()
            && result.cards.is_empty()
            && page.get_untracked() > 0
        {
            page.update(|p| *p -= 1);
        }
    });

    view! {
        <h1>"Cards"</h1>
        <div class="card-filters">
            // Previously only reachable via the nav bar's "Cards ▾" dropdown
            // — confirmed live that was the only way in, one extra hop from
            // the page you're already looking at the cards on.
            <A href="/new" attr:class="card-filters-add">"Add a card"</A>
            <label>
                "Filter by tag "
                <select
                    prop:value=move || tag_filter.get().unwrap_or_default()
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        tag_filter.set(if value.is_empty() { None } else { Some(value) });
                        page.set(0);
                    }
                >
                    <option value="">"All tags"</option>
                    <Transition fallback=|| ()>
                        {move || Suspend::new(async move {
                            tags.await.unwrap_or_default().into_iter().map(|tag| {
                                view! { <option value=tag.clone()>{tag.clone()}</option> }
                            }).collect_view()
                        })}
                    </Transition>
                </select>
            </label>
            " "
            <label>
                "Filter by deck "
                <select
                    prop:value=move || {
                        deck_filter.get().map(|id| id.to_string()).unwrap_or_default()
                    }
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        deck_filter.set(Uuid::parse_str(&value).ok());
                        page.set(0);
                    }
                >
                    <option value="">"All decks"</option>
                    <Transition fallback=|| ()>
                        {move || Suspend::new(async move {
                            decks.await.unwrap_or_default().into_iter().map(|deck| {
                                view! { <option value=deck.id.to_string()>{deck.name}</option> }
                            }).collect_view()
                        })}
                    </Transition>
                </select>
            </label>
        </div>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                match cards.await {
                    Ok(result) if result.cards.is_empty() => {
                        // Distinguish "nothing matches this filter" from
                        // "you have zero cards at all" — only the latter
                        // gets a call to action, since a narrow filter was
                        // the user's own deliberate choice.
                        let unfiltered = tag_filter.get_untracked().is_none()
                            && deck_filter.get_untracked().is_none();
                        if unfiltered {
                            view! {
                                <p>"No cards yet. " <A href="/new">"Add the first one"</A>"."</p>
                            }
                            .into_any()
                        } else {
                            view! { <p>"No cards match these filters."</p> }.into_any()
                        }
                    }
                    Ok(result) => {
                        let total = result.total;
                        let current_page = page.get_untracked();
                        let total_pages = ((total + CARDS_PAGE_SIZE - 1) / CARDS_PAGE_SIZE).max(1);
                        let has_more = (current_page + 1) * CARDS_PAGE_SIZE < total;
                        view! {
                        <ul class="card-grid">
                            {result.cards
                                .into_iter()
                                .map(|card| {
                                    let edit_href = format!("/cards/{}", card.id);
                                    view! {
                                        <li>
                                            {card.source_text.map(|text| view! { <p>{text}</p> })}
                                            {card
                                                .image_url
                                                .map(|url| view! { <img src=url alt="" /> })}
                                            {card
                                                .audio_url
                                                .map(|url| view! { <audio controls src=url></audio> })}
                                            {card
                                                .video_url
                                                .map(|url| view! { <video controls src=url></video> })}
                                            {card.youtube_url.map(youtube_embed)}
                                            // The grid is a browsing/self-test view — only the
                                            // prompt (media, if any — or source_text for a
                                            // sentence_translation card) shows up front, same
                                            // as ReviewPage and EditCardPage's signed-out view.
                                            // target_text is the answer, behind Reveal.
                                            <details>
                                                <summary>"Reveal"</summary>
                                                <span>{card.target_text}</span>
                                            </details>
                                            <div class="card-grid-actions">
                                                <A href=edit_href>"Edit"</A>
                                                // Deliberately a single click,
                                                // no arm-then-confirm step —
                                                // asked for directly, and
                                                // matches this app's existing
                                                // "acceptable for 4 trusted
                                                // users" tolerance for low
                                                // ceremony elsewhere (e.g.
                                                // EditCardPage's own delete
                                                // also dispatches immediately
                                                // once armed; this just drops
                                                // the arm step for the
                                                // one-tap case this grid view
                                                // is for).
                                                <button
                                                    type="button"
                                                    on:click=move |_| {
                                                        delete.dispatch(DeleteCard { card_id: card.id });
                                                    }
                                                >
                                                    "Delete"
                                                </button>
                                            </div>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                        {(total_pages > 1).then(|| view! {
                            <div class="card-pager">
                                <button
                                    type="button"
                                    disabled=current_page == 0
                                    on:click=move |_| page.update(|p| *p -= 1)
                                >
                                    "Previous"
                                </button>
                                <span>{format!("Page {} of {total_pages}", current_page + 1)}</span>
                                <button
                                    type="button"
                                    disabled=!has_more
                                    on:click=move |_| page.update(|p| *p += 1)
                                >
                                    "Next"
                                </button>
                            </div>
                        })}
                    }
                    .into_any()
                    }
                    Err(e) => view! { <p>"Error loading cards: " {e.to_string()}</p> }.into_any(),
                }
            })}
        </Transition>
    }
}

#[component]
fn NewCardPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| current_user_name());
    // Created alongside `auth`, before the Suspend below, so it fetches in
    // parallel rather than serialized behind auth resolving first — same
    // discipline `EditCardPage` uses for its own `decks`/`pages` resources.
    let decks = Resource::new(|| (), |()| list_decks());
    let audio_input: NodeRef<html::Input> = NodeRef::new();
    let video_input: NodeRef<html::Input> = NodeRef::new();
    let chosen_image_url: RwSignal<Option<String>> = RwSignal::new(None);
    let target_text_signal: RwSignal<String> = RwSignal::new(String::new());
    let card_type = RwSignal::new("image_to_target".to_string());
    // Confirmed live: the "Dictionary details" section always started
    // blank, even typing a word that already has a saved entry — e.g. a
    // second card for an already-documented word (an audio-prompt card
    // for "umfundi" after an image-prompt one already exists), where the
    // definition should just already be there, not need retyping. Looked
    // up once, on leaving the target_text field, rather than on every
    // keystroke (unlike wiki_sidebar's search, a remount here would wipe
    // out whatever the user had already started typing into the
    // dictionary fields themselves) — word_looked_up gates it to that one
    // shot, and word_lookup only ever moves None -> Some, so the
    // CardDictionarySection below only remounts if a match is actually
    // found.
    let word_lookup: RwSignal<Option<WordDefinition>> = RwSignal::new(None);
    let word_looked_up = RwSignal::new(false);

    view! {
        <h1>"Add a card"</h1>
        {move || form_error_banner()}
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                if auth.await.ok().flatten().is_none() {
                    return view! { <Redirect path="/login"/> }.into_any();
                }
                view! {
                    <Form method="post" action="/cards/new" enctype="multipart/form-data".to_string()>
                        <label>
                            {move || if card_type.get() == "sentence_translation" {
                                "Target-language sentence"
                            } else {
                                "Target-language word/phrase"
                            }}
                            <input
                                type="text"
                                name="target_text"
                                on:input=move |ev| target_text_signal.set(event_target_value(&ev))
                                on:blur=move |_| {
                                    if word_looked_up.get_untracked() {
                                        return;
                                    }
                                    let text = target_text_signal.get_untracked();
                                    if text.trim().is_empty() {
                                        return;
                                    }
                                    word_looked_up.set(true);
                                    #[cfg(feature = "hydrate")]
                                    wasm_bindgen_futures::spawn_local(async move {
                                        if let Ok(Some(def)) = get_word_definition(text).await {
                                            word_lookup.set(Some(def));
                                        }
                                    });
                                }
                            />
                        </label>
                        <label>"Tags (comma separated)" <input type="text" name="tags" /></label>
                        <label>
                            "Card type "
                            <select
                                name="card_type"
                                prop:value=move || card_type.get()
                                on:change=move |ev| card_type.set(event_target_value(&ev))
                            >
                                {CARD_TYPES.iter().map(|(value, label)| {
                                    view! { <option value=*value>{*label}</option> }
                                }).collect_view()}
                            </select>
                        </label>
                        {move || {
                            if card_type.get() == "sentence_translation" {
                                view! {
                                    <label>
                                        "Known-language sentence"
                                        <input type="text" name="source_text" />
                                    </label>
                                }
                                .into_any()
                            } else {
                                view! {
                                    <MediaAttachmentFields
                                        chosen_image_url=chosen_image_url
                                        audio_input=audio_input
                                        video_input=video_input
                                        fields=media_fields_for(&card_type.get())
                                    />
                                }
                                .into_any()
                            }
                        }}
                        <label>
                            <input type="checkbox" name="two_way" />
                            "Two-way: also review back → front, scheduled independently"
                        </label>
                        {move || view! {
                            <CardDictionarySection
                                target_text_signal=target_text_signal
                                existing_definition=word_lookup.get()
                                embedded=true
                            />
                        }}
                        // Unchecked by default — there's no existing
                        // membership to reflect for a card that doesn't
                        // exist yet. Chosen deck ids ride this same
                        // multipart POST (see the "deck_id" arm in
                        // main.rs's parse_card_multipart) rather than a
                        // separate step, since there's no card id to
                        // dispatch a deck-toggle action against until the
                        // handler creates one.
                        <div class="card-section">
                            <p class="card-section-heading">"Decks"</p>
                            <Transition fallback=|| view! { <p>"Loading decks…"</p> }>
                                {move || Suspend::new(async move {
                                    match decks.await {
                                        Ok(decks) => decks
                                            .into_iter()
                                            .map(|deck| {
                                                view! {
                                                    <label>
                                                        <input
                                                            type="checkbox"
                                                            name="deck_id"
                                                            value=deck.id.to_string()
                                                        />
                                                        {deck.name}
                                                    </label>
                                                }
                                            })
                                            .collect_view()
                                            .into_any(),
                                        Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                                    }
                                })}
                            </Transition>
                        </div>
                        <button type="submit">"Add"</button>
                    </Form>
                    // No card_id exists until this form is submitted, so
                    // there's nothing a link control could attach to yet —
                    // this previews the three LINK_TYPES as inert pills
                    // rather than a control with nothing to do. The real
                    // "Related cards" UI lives on EditCardPage, once a
                    // card_id exists (see CardLinksSection).
                    <div class="card-section xref-teaser">
                        <p class="card-section-heading">"Cross-references"</p>
                        <p>
                            "Minimal pairs, sound-spelling correspondences, and back-chain steps are linked once this card has been saved and filed — from its own page."
                        </p>
                        <div class="xref-ghost-row">
                            {LINK_TYPES.iter().map(|(_, label, _)| {
                                view! { <span class="xref-ghost">{*label}</span> }
                            }).collect_view()}
                        </div>
                    </div>
                }
                .into_any()
            })}
        </Transition>
    }
}

/// One parsed line from the pasted list, waiting on the page for an image
/// before the batch is submitted. `key` is stable across this page's
/// lifetime (assigned once at parse time, never reused) so each draft's
/// own `ImageSearchPicker` keeps its chosen image if anything elsewhere on
/// the page re-renders.
#[derive(Debug, Clone)]
struct BulkDraft {
    key: usize,
    english: String,
    target_text: String,
}

/// Parses one `english,target word` line. The first comma splits english
/// from the target phrase; anything after a *second* comma stays part of
/// the target phrase rather than being treated as a parse error, so a
/// target phrase that itself contains a comma still works. `None` means
/// the line didn't look like "english,target" at all (missing comma, or
/// either side empty after trimming).
fn parse_bulk_line(line: &str) -> Option<(String, String)> {
    let (english, target) = line.trim().split_once(',')?;
    let english = english.trim();
    let target = target.trim();
    (!english.is_empty() && !target.is_empty()).then(|| (english.to_string(), target.to_string()))
}

#[component]
fn BulkAddPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| current_user_name());
    let raw_input = RwSignal::new(String::new());
    let drafts: RwSignal<Vec<BulkDraft>> = RwSignal::new(Vec::new());
    let skipped_lines: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let next_key = RwSignal::new(0usize);

    // Parsing replaces `drafts` wholesale, which would also wipe out any
    // images already chosen on a re-parse — deliberately only ever
    // triggered by this explicit click, never reactively on every
    // keystroke, so starting to pick images is a safe point to stop
    // touching the textarea.
    let parse = move |_| {
        let mut parsed = Vec::new();
        let mut skipped = Vec::new();
        for line in raw_input.get_untracked().lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match parse_bulk_line(line) {
                Some((english, target_text)) => {
                    let key = next_key.get_untracked();
                    next_key.set(key + 1);
                    parsed.push(BulkDraft {
                        key,
                        english,
                        target_text,
                    });
                }
                None => skipped.push(line.to_string()),
            }
        }
        drafts.set(parsed);
        skipped_lines.set(skipped);
    };

    view! {
        <h1>"Bulk add cards"</h1>
        <p class="page-intro">
            "Faster than adding one at a time — paste a whole list, check the parsed results, then create them together."
        </p>
        {move || form_error_banner()}
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                if auth.await.ok().flatten().is_none() {
                    return view! { <Redirect path="/login"/> }.into_any();
                }
                view! {
                    <label>
                        "Paste one card per line: english,target word"
                        <textarea
                            rows="8"
                            on:input=move |ev| raw_input.set(event_target_value(&ev))
                        ></textarea>
                    </label>
                    <button type="button" on:click=parse>"Parse"</button>
                    {move || {
                        let skipped = skipped_lines.get();
                        (!skipped.is_empty()).then(|| {
                            let count = skipped.len();
                            let lines = skipped.join("; ");
                            view! {
                                <p>
                                    "Skipped " {count}
                                    " line(s) that weren't \"english,word\": " {lines}
                                </p>
                            }
                        })
                    }}
                    <Form method="post" action="/cards/bulk" enctype="multipart/form-data".to_string()>
                        {move || {
                            drafts
                                .get()
                                .into_iter()
                                .map(|draft| {
                                    let chosen_image_url: RwSignal<Option<String>> = RwSignal::new(None);
                                    let audio_input: NodeRef<html::Input> = NodeRef::new();
                                    let video_input: NodeRef<html::Input> = NodeRef::new();
                                    let card_type = RwSignal::new("image_to_target".to_string());
                                    let idx = draft.key;
                                    let suffix = format!("_{idx}");
                                    view! {
                                        // .card-section gives each draft the same
                                        // bordered-box treatment NewCardPage's
                                        // "Decks" block already uses — confirmed
                                        // live that without it, a page of several
                                        // drafts ran together into one
                                        // undifferentiated wall of fields with no
                                        // visual boundary between "this card" and
                                        // "the next one".
                                        <div class="card-section bulk-draft">
                                            <p class="card-section-heading">
                                                {draft.target_text.clone()} " (" {draft.english.clone()} ")"
                                            </p>
                                            <input
                                                type="hidden"
                                                name=format!("target_text{suffix}")
                                                value=draft.target_text.clone()
                                            />
                                            <label>
                                                "Card type "
                                                <select
                                                    name=format!("card_type{suffix}")
                                                    prop:value=move || card_type.get()
                                                    on:change=move |ev| card_type.set(event_target_value(&ev))
                                                >
                                                    {CARD_TYPES.iter().map(|(value, label)| {
                                                        view! { <option value=*value>{*label}</option> }
                                                    }).collect_view()}
                                                </select>
                                            </label>
                                            {move || {
                                                if card_type.get() == "sentence_translation" {
                                                    view! {
                                                        <label>
                                                            "Known-language sentence"
                                                            <input
                                                                type="text"
                                                                name=format!("source_text{suffix}")
                                                            />
                                                        </label>
                                                    }
                                                        .into_any()
                                                } else {
                                                    view! {
                                                        <MediaAttachmentFields
                                                            chosen_image_url=chosen_image_url
                                                            audio_input=audio_input
                                                            video_input=video_input
                                                            fields=media_fields_for(&card_type.get())
                                                            field_suffix=suffix.clone()
                                                            search_query=draft.english.clone()
                                                        />
                                                    }
                                                        .into_any()
                                                }
                                            }}
                                        </div>
                                    }
                                })
                                .collect_view()
                        }}
                        {move || {
                            (!drafts.get().is_empty())
                                .then(|| view! { <button type="submit">"Create all cards"</button> })
                        }}
                    </Form>
                }
                .into_any()
            })}
        </Transition>
    }
}

/// The read-only media + word preview shown above `EditCardPage`'s edit
/// form — pulled out of its view (pure decomposition, no behavior change),
/// since the two are only ever rendered together inside one
/// `.review-card` block.
#[component]
fn CardPreviewHeader(
    card_type: String,
    image_url: Option<String>,
    audio_url: Option<String>,
    video_url: Option<String>,
    youtube_url: Option<String>,
    target_text: String,
    // `sentence_translation` cards only — fills the same "prompt" role a
    // media_view would for every other card type.
    source_text: Option<String>,
    tags: Vec<String>,
    signed_in: bool,
) -> impl IntoView {
    let image_view = image_url.map(|url| view! { <img src=url alt="" /> });
    // image_to_audio: audio is the answer, not the prompt — same
    // prompt/reveal split ReviewPage's own card_type == "image_to_audio"
    // branch uses, and for the same reason (showing it alongside the image
    // up front would leak the answer before "Reveal"). Every other type
    // shows audio immediately, alongside image/video/youtube, since it's
    // part of *their* prompt.
    let audio_view = audio_url.map(|url| view! { <audio controls src=url></audio> });
    let (prompt_audio, answer_audio) = if card_type == "image_to_audio" {
        (None, audio_view)
    } else {
        (audio_view, None)
    };
    let video_view = video_url.map(|url| view! { <video controls src=url></video> });
    let youtube_view = youtube_url.map(youtube_embed);
    let source_view = source_text.map(|text| view! { <p>{text}</p> });
    let tags_display = tags.join(", ");
    let info = view! {
        {answer_audio}
        <div class="word-display"><ClickableWord text=target_text /></div>
        {(!tags.is_empty()).then(|| view! { <p>"Tags: " {tags_display}</p> })}
    };
    // A signed-in editor sees the word immediately — visual confirmation of
    // which card this is before the (possibly media-heavy) form below it. A
    // signed-out visitor instead gets the same reveal-then-show shape as
    // ReviewPage's forward direction: media is the prompt, the word is
    // hidden behind "Reveal". Browsing a card to self-test is open to
    // anyone; it's only *grading* (ReviewPage's Again/Hard/Good/Easy, which
    // writes to the Anki-derived SRS state) that requires an account —
    // there's no grading UI here at all.
    let word_preview = if signed_in {
        info.into_any()
    } else {
        view! {
            <details>
                <summary>"Reveal"</summary>
                {info}
            </details>
        }
        .into_any()
    };
    view! {
        <div class="review-card">
            {image_view}
            {prompt_audio}
            {video_view}
            {youtube_view}
            {source_view}
            {word_preview}
        </div>
    }
}

/// Thin wrapper around `DictionaryDetailsEditor` for `EditCardPage` — pulled
/// out of its view (pure decomposition, no behavior change). The
/// `get_word_definition` fetch stays in the parent (already sequential on
/// `detail` there, nothing to run in parallel by moving it here) — this
/// just takes the already-resolved result and builds the editor's
/// `*_initial` props from it.
#[component]
fn CardDictionarySection(
    target_text_signal: RwSignal<String>,
    existing_definition: Option<WordDefinition>,
    #[prop(default = false)] embedded: bool,
) -> impl IntoView {
    let first_example = existing_definition
        .as_ref()
        .and_then(|d| d.examples.first());
    view! {
        <DictionaryDetailsEditor
            target_text=target_text_signal.into()
            embedded=embedded
            part_of_speech_initial=existing_definition
                .as_ref()
                .and_then(|d| d.part_of_speech.clone())
                .unwrap_or_default()
            notes_initial=existing_definition
                .as_ref()
                .and_then(|d| d.notes.clone())
                .unwrap_or_default()
            english_translation_initial=existing_definition
                .as_ref()
                .and_then(|d| d.english_translation.clone())
                .unwrap_or_default()
            example_target_initial=first_example
                .map(|e| e.target_sentence.clone())
                .unwrap_or_default()
            example_gloss_initial=first_example
                .and_then(|e| e.gloss_sentence.clone())
                .unwrap_or_default()
            linked_words_initial=existing_definition
                .as_ref()
                .map(|d| {
                    d.linked_words
                        .iter()
                        .map(|l| l.target_text.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
        />
    }
}

/// `EditCardPage`'s multipart save form — pulled out of its view (pure
/// decomposition, no behavior change beyond what `card_refresh` below
/// fixes). Owns `chosen_image_url`/`audio_input`/`video_input` itself, the
/// same way `NewCardPage` already owns its own copies, since nothing
/// outside this form needs them.
///
/// `card_refresh` is `EditCardPage`'s `card` Resource-refetch signal,
/// wired to `<Form>`'s own `version` prop (bumped on every completed
/// submit). A plain `<form>` POST used to make this unnecessary — its
/// hard page reload re-fetched everything from scratch regardless. `<Form>`
/// instead redirect-follows via a client-side `navigate()`, which does
/// nothing here: saving redirects back to this same `/cards/{id}` route,
/// so `card`'s Resource (keyed only on the id, which hasn't changed) would
/// never refetch and the page would keep showing pre-save data — confirmed
/// live (a YouTube-link removal visibly failed to clear the embed after
/// Save, with the DB write itself having actually succeeded).
#[component]
fn CardEditForm(
    id: Uuid,
    target_text: String,
    source_text: Option<String>,
    tags: Vec<String>,
    card_type: String,
    two_way: bool,
    has_image: bool,
    has_audio: bool,
    has_video: bool,
    has_youtube: bool,
    target_text_signal: RwSignal<String>,
    card_refresh: RwSignal<usize>,
) -> impl IntoView {
    let audio_input: NodeRef<html::Input> = NodeRef::new();
    let video_input: NodeRef<html::Input> = NodeRef::new();
    let chosen_image_url: RwSignal<Option<String>> = RwSignal::new(None);
    let action = format!("/cards/{id}");
    let tags_value = tags.join(", ");
    let card_type_signal = RwSignal::new(card_type);
    let source_text_value = source_text.unwrap_or_default();
    view! {
        <Form
            method="post"
            action=action
            enctype="multipart/form-data".to_string()
            version=card_refresh
        >
            <label>
                {move || if card_type_signal.get() == "sentence_translation" {
                    "Target-language sentence"
                } else {
                    "Target-language word/phrase"
                }}
                <input
                    type="text"
                    name="target_text"
                    value=target_text
                    on:input=move |ev| target_text_signal.set(event_target_value(&ev))
                />
            </label>
            <label>"Tags (comma separated)" <input type="text" name="tags" value=tags_value /></label>
            <label>
                "Card type "
                <select
                    name="card_type"
                    prop:value=move || card_type_signal.get()
                    on:change=move |ev| card_type_signal.set(event_target_value(&ev))
                >
                    {CARD_TYPES.iter().map(|(value, label)| {
                        view! { <option value=*value>{*label}</option> }
                    }).collect_view()}
                </select>
            </label>
            {move || {
                if card_type_signal.get() == "sentence_translation" {
                    view! {
                        <label>
                            "Known-language sentence"
                            <input type="text" name="source_text" value=source_text_value.clone() />
                        </label>
                    }
                    .into_any()
                } else {
                    // Narrowed from the type's nominal MediaFields only
                    // when nothing already set would end up hidden with no
                    // other way left on this form to see or remove it —
                    // e.g. a card that's (say) sound_to_written but already
                    // carries an image from before this type existed.
                    // Video/youtube need no such check: MediaAttachmentFields
                    // always offers them regardless of `fields`.
                    let card_type = card_type_signal.get();
                    let nominal = media_fields_for(&card_type);
                    let fields = match nominal {
                        MediaFields::Audio if has_image => MediaFields::ImageAndAudio,
                        MediaFields::Image if has_audio => MediaFields::ImageAndAudio,
                        other => other,
                    };
                    view! {
                        <MediaAttachmentFields
                            editing=true
                            chosen_image_url=chosen_image_url
                            audio_input=audio_input
                            video_input=video_input
                            has_image=has_image
                            has_audio=has_audio
                            has_video=has_video
                            has_youtube=has_youtube
                            fields=fields
                        />
                    }
                    .into_any()
                }
            }}
            <label>
                <input type="checkbox" name="two_way" checked=two_way />
                "Two-way: also review back → front, scheduled independently"
            </label>
            <button type="submit">"Save"</button>
        </Form>
    }
}

/// The "Decks" section of `EditCardPage` — pulled out of its view (pure
/// decomposition, no behavior change). `decks` is created in the parent
/// and passed in rather than fetched here: `EditCardPage` creates it before
/// the per-card `Suspend` so it loads in parallel with `card`/`auth` —
/// fetching it here instead would serialize it behind `card` resolving
/// first.
#[component]
fn DeckAssignmentSection(
    card_id: Uuid,
    deck_ids: Vec<Uuid>,
    signed_in: bool,
    decks: Resource<Result<Vec<Deck>, ServerFnError>>,
    toggle_deck: ServerAction<AddCardToDeck>,
    untoggle_deck: ServerAction<RemoveCardFromDeck>,
) -> impl IntoView {
    view! {
        <div class="card-section">
            <p class="card-section-heading">"Decks"</p>
            <Transition fallback=|| view! { <p>"Loading decks…"</p> }>
            {move || {
                let deck_ids = deck_ids.clone();
                Suspend::new(async move {
                    match decks.await {
                        Ok(decks) if signed_in => decks
                            .into_iter()
                            .map(|deck| {
                                let in_deck = deck_ids.contains(&deck.id);
                                let deck_id = deck.id;
                                view! {
                                    <label>
                                        <input
                                            type="checkbox"
                                            checked=in_deck
                                            on:change=move |ev| {
                                                if event_target_checked(&ev) {
                                                    toggle_deck.dispatch(AddCardToDeck { deck_id, card_id });
                                                } else {
                                                    untoggle_deck.dispatch(RemoveCardFromDeck { deck_id, card_id });
                                                }
                                            }
                                        />
                                        {deck.name}
                                    </label>
                                }
                            })
                            .collect_view()
                            .into_any(),
                        Ok(decks) => {
                            let names: Vec<_> = decks
                                .into_iter()
                                .filter(|d| deck_ids.contains(&d.id))
                                .map(|d| d.name)
                                .collect();
                            if names.is_empty() {
                                view! { <p>"Not in any deck."</p> }.into_any()
                            } else {
                                view! { <p>{names.join(", ")}</p> }.into_any()
                            }
                        }
                        Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                    }
                })
            }}
            </Transition>
        </div>
    }
}

/// The "Grammar pages" section of `EditCardPage` — pulled out of its view
/// (pure decomposition, no behavior change). Same parallel-fetch reasoning
/// as `DeckAssignmentSection`: `pages` is created in the parent and passed
/// in, not fetched here.
#[component]
fn PageAssignmentSection(
    card_id: Uuid,
    page_ids: Vec<Uuid>,
    signed_in: bool,
    pages: Resource<Result<Vec<PageTreeNode>, ServerFnError>>,
    toggle_page: ServerAction<AddPageToCard>,
    untoggle_page: ServerAction<RemovePageFromCard>,
) -> impl IntoView {
    view! {
        <div class="card-section">
            <p class="card-section-heading">"Grammar pages"</p>
            <Transition fallback=|| view! { <p>"Loading pages…"</p> }>
            {move || {
                let page_ids = page_ids.clone();
                Suspend::new(async move {
                    match pages.await {
                        Ok(pages) if signed_in => {
                            let mut labeled: Vec<_> = pages
                                .iter()
                                .map(|p| (p.id, page_path_label(&pages, p.id)))
                                .collect();
                            labeled.sort_by(|a, b| a.1.cmp(&b.1));
                            labeled
                                .into_iter()
                                .map(|(page_id, label)| {
                                    let on_page = page_ids.contains(&page_id);
                                    view! {
                                        <label>
                                            <input
                                                type="checkbox"
                                                checked=on_page
                                                on:change=move |ev| {
                                                    if event_target_checked(&ev) {
                                                        toggle_page.dispatch(AddPageToCard { card_id, page_id });
                                                    } else {
                                                        untoggle_page.dispatch(RemovePageFromCard { card_id, page_id });
                                                    }
                                                }
                                            />
                                            {label}
                                        </label>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }
                        Ok(pages) => {
                            let labels: Vec<_> = pages
                                .iter()
                                .filter(|p| page_ids.contains(&p.id))
                                .map(|p| page_path_label(&pages, p.id))
                                .collect();
                            if labels.is_empty() {
                                view! { <p>"Not linked to any grammar page."</p> }.into_any()
                            } else {
                                view! { <p>{labels.join(", ")}</p> }.into_any()
                            }
                        }
                        Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                    }
                })
            }}
            </Transition>
        </div>
    }
}

fn link_type_label(value: &str) -> &'static str {
    LINK_TYPES
        .iter()
        .find(|(v, _, _)| *v == value)
        .map_or("Unknown", |(_, label, _)| label)
}

fn link_type_is_directional(value: &str) -> bool {
    LINK_TYPES
        .iter()
        .find(|(v, _, _)| *v == value)
        .is_some_and(|(_, _, directional)| *directional)
}

/// The "Related cards" section of `EditCardPage` — typed links to other
/// specific cards (minimal pairs, back-chain steps, sound-spelling
/// correspondences), distinct from the shared-label grouping tags/decks/
/// pages already provide. See `cards::LINK_TYPES` for the known types and
/// which one is directional. Live-search-as-you-type re-fetches on every
/// keystroke rather than debouncing, same precedent as the wiki sidebar's
/// page search — fine at this app's scale.
#[component]
fn CardLinksSection(
    card_id: Uuid,
    linked_cards: Vec<CardLink>,
    signed_in: bool,
    add_link: ServerAction<AddCardLink>,
    remove_link: ServerAction<RemoveCardLink>,
) -> impl IntoView {
    let link_type = RwSignal::new(LINK_TYPES[0].0.to_string());
    // Only consulted for the one directional link type — which card goes
    // in `card_id` vs `linked_card_id` on dispatch.
    let comes_before = RwSignal::new(true);
    let search_query = RwSignal::new(String::new());
    let search_results = Resource::new(
        move || search_query.get(),
        move |q| async move {
            let q = q.trim().to_string();
            if q.is_empty() {
                Ok(Vec::new())
            } else {
                search_cards(q, card_id).await
            }
        },
    );

    // Computed from linked_cards by reference before the list below
    // consumes it, so the legend reflects exactly what's in the list —
    // not a separate query that could drift from it.
    let legend = LINK_TYPES
        .iter()
        .map(|(value, label, _)| {
            let count = linked_cards
                .iter()
                .filter(|l| l.link_type == *value)
                .count();
            view! { <span class="link-type-count">{*label}" "<em>{count}</em></span> }
        })
        .collect_view();

    let links_list = if linked_cards.is_empty() {
        view! { <p class="card-links-empty">"Not linked to any other card."</p> }.into_any()
    } else {
        view! {
            <ul class="card-links-list">
                {linked_cards
                    .into_iter()
                    .map(|link| {
                        let label = link_type_label(&link.link_type);
                        let arrow = if link_type_is_directional(&link.link_type) {
                            if link.is_source { "→ " } else { "← " }
                        } else {
                            ""
                        };
                        let linked_card_id = link.linked_card_id;
                        let remove_link_type = link.link_type.clone();
                        view! {
                            <li>
                                <span class="link-arrow">{arrow}</span>
                                <span class="link-word">{link.target_text}</span>
                                <span class="link-type-badge">{label}</span>
                                {signed_in.then(|| {
                                    let remove_link_type = remove_link_type.clone();
                                    view! {
                                        " "
                                        <button
                                            type="button"
                                            on:click=move |_| {
                                                remove_link.dispatch(RemoveCardLink {
                                                    card_id,
                                                    linked_card_id,
                                                    link_type: remove_link_type.clone(),
                                                });
                                            }
                                        >
                                            "Remove"
                                        </button>
                                    }
                                })}
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        }
        .into_any()
    };

    let add_form = signed_in.then(|| {
        view! {
            <div class="card-links-add">
                <label>
                    "Link type "
                    <select
                        prop:value=move || link_type.get()
                        on:change=move |ev| link_type.set(event_target_value(&ev))
                    >
                        {LINK_TYPES.iter().map(|(value, label, _)| {
                            view! { <option value=*value>{*label}</option> }
                        }).collect_view()}
                    </select>
                </label>
                {move || link_type_is_directional(&link_type.get()).then(|| view! {
                    <span>
                        <label>
                            <input
                                type="radio"
                                name="card-link-direction"
                                prop:checked=move || comes_before.get()
                                on:change=move |_| comes_before.set(true)
                            />
                            "This card comes before the one I pick"
                        </label>
                        " "
                        <label>
                            <input
                                type="radio"
                                name="card-link-direction"
                                prop:checked=move || !comes_before.get()
                                on:change=move |_| comes_before.set(false)
                            />
                            "Comes after"
                        </label>
                    </span>
                })}
                <br />
                <input
                    type="search"
                    placeholder="Search cards to link…"
                    prop:value=move || search_query.get()
                    on:input=move |ev| search_query.set(event_target_value(&ev))
                />
                <Transition fallback=|| ()>
                    {move || Suspend::new(async move {
                        match search_results.await {
                            Ok(results) => results
                                .into_iter()
                                .map(|result| {
                                    let picked_id = result.id;
                                    view! {
                                        <button
                                            type="button"
                                            on:click=move |_| {
                                                let (from, to) = if comes_before.get() {
                                                    (card_id, picked_id)
                                                } else {
                                                    (picked_id, card_id)
                                                };
                                                add_link.dispatch(AddCardLink {
                                                    card_id: from,
                                                    linked_card_id: to,
                                                    link_type: link_type.get(),
                                                });
                                                search_query.set(String::new());
                                            }
                                        >
                                            {result.target_text.clone()}
                                        </button>
                                    }
                                })
                                .collect_view()
                                .into_any(),
                            Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                        }
                    })}
                </Transition>
            </div>
        }
    });

    view! {
        <div class="card-section">
            <p class="card-section-heading">"Related cards"</p>
            <div class="card-links-legend">{legend}</div>
            {links_list}
            {add_form}
        </div>
    }
}

/// The delete button + its own dispatch error — pulled out of
/// `EditCardPage`'s view (pure decomposition, no behavior change). The
/// `delete` action's navigate-on-success `Effect` stays in the parent,
/// exactly as today — it doesn't live next to this form there either.
#[component]
fn DeleteCardSection(card_id: Uuid, delete: ServerAction<DeleteCard>) -> impl IntoView {
    let armed = RwSignal::new(false);
    view! {
        <ActionForm action=delete>
            <input type="hidden" name="card_id" value=card_id.to_string() />
            {confirm_button("Delete card", armed)}
        </ActionForm>
        {move || {
            delete.value().get().and_then(Result::err).map(|e| {
                view! { <p>"Error: " {e.to_string()}</p> }
            })
        }}
    }
}

#[component]
fn EditCardPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| current_user_name());
    let params = use_params_map();
    let card_id = move || {
        params
            .read()
            .get("id")
            .and_then(|id| Uuid::parse_str(&id).ok())
    };

    // Bumped by `CardEditForm`'s `<Form version=card_refresh>` on every
    // completed save — see that component's doc comment for why this is
    // needed now that saving no longer hard-reloads the page.
    let card_refresh = RwSignal::new(0usize);
    let add_link = ServerAction::<AddCardLink>::new();
    let remove_link = ServerAction::<RemoveCardLink>::new();
    // Unlike deck/page checkboxes (whose checked-ness falls back on native
    // DOM state after a click, so they never needed this), the "Related
    // cards" list is plain rendered content re-derived from `card` each
    // time — it has no such fallback, so add_link/remove_link's own
    // version signals are folded into this Resource's key too, same
    // precedent as ReviewPage keying its queue on `grade.version()`.
    let card = Resource::new(
        move || {
            (
                card_id(),
                card_refresh.get(),
                add_link.version().get(),
                remove_link.version().get(),
            )
        },
        |(id, ..)| async move {
            match id {
                Some(id) => get_card(id).await,
                None => Ok(None),
            }
        },
    );
    let decks = Resource::new(|| (), |()| list_decks());
    let pages = Resource::new(|| (), |()| list_page_tree());

    let delete = ServerAction::<DeleteCard>::new();
    let toggle_deck = ServerAction::<AddCardToDeck>::new();
    let untoggle_deck = ServerAction::<RemoveCardFromDeck>::new();
    let toggle_page = ServerAction::<AddPageToCard>::new();
    let untoggle_page = ServerAction::<RemovePageFromCard>::new();

    let navigate = use_navigate();
    Effect::new(move |_| {
        // Was `.is_some()` — true on a *failed* delete too, so the page
        // navigated away and silently hid the error. Only a real success
        // should leave this page.
        if matches!(delete.value().get(), Some(Ok(()))) {
            navigate("/", leptos_router::NavigateOptions::default());
        }
    });

    view! {
        <h1>"Card"</h1>
        {move || form_error_banner()}
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                let Some(id) = card_id() else {
                    return view! { <p>"Invalid card."</p> }.into_any();
                };
                let signed_in = auth.await.ok().flatten().is_some();
                match card.await {
                    Ok(None) => view! { <p>"Card not found."</p> }.into_any(),
                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                    Ok(Some(detail)) => {
                        let deck_ids = detail.deck_ids.clone();
                        let page_ids = detail.page_ids.clone();

                        // Tracks the input's *current* value reactively, so
                        // the "Dictionary details" editor below always saves
                        // against whatever's actually typed — including a
                        // just-edited rename not yet submitted — rather than
                        // the stale value this page loaded with.
                        let target_text_signal = RwSignal::new(detail.target_text.clone());
                        let existing_definition = get_word_definition(detail.target_text.clone())
                            .await
                            .ok()
                            .flatten();

                        // Every child component here returns `impl IntoView`,
                        // not `AnyView` — fine individually, but composing
                        // all seven nested types inline into this one view!
                        // overflowed rustc's query-depth limit in a release
                        // build (confirmed live: "queries overflow the depth
                        // limit!", naming this exact closure). `.into_any()`
                        // on each caps the type at this boundary instead of
                        // inlining it further — same fix already used by
                        // `wiki_sidebar`/`wiki_tree_level` above for the same
                        // failure mode, see that doc comment for the history.
                        let preview = view! {
                            <CardPreviewHeader
                                card_type=detail.card_type.clone()
                                image_url=detail.image_url.clone()
                                audio_url=detail.audio_url.clone()
                                video_url=detail.video_url.clone()
                                youtube_url=detail.youtube_url.clone()
                                target_text=detail.target_text.clone()
                                source_text=detail.source_text.clone()
                                tags=detail.tags.clone()
                                signed_in=signed_in
                            />
                        }
                        .into_any();
                        let dictionary_editor = signed_in.then(|| {
                            view! {
                                <CardDictionarySection
                                    target_text_signal=target_text_signal
                                    existing_definition=existing_definition
                                />
                            }
                            .into_any()
                        });
                        let edit_form = signed_in.then(|| {
                            view! {
                                <CardEditForm
                                    id=id
                                    target_text=detail.target_text.clone()
                                    source_text=detail.source_text.clone()
                                    tags=detail.tags.clone()
                                    card_type=detail.card_type.clone()
                                    two_way=detail.two_way
                                    has_image=detail.image_url.is_some()
                                    has_audio=detail.audio_url.is_some()
                                    has_video=detail.video_url.is_some()
                                    has_youtube=detail.youtube_url.is_some()
                                    target_text_signal=target_text_signal
                                    card_refresh=card_refresh
                                />
                            }
                            .into_any()
                        });
                        let deck_section = view! {
                            <DeckAssignmentSection
                                card_id=id
                                deck_ids=deck_ids
                                signed_in=signed_in
                                decks=decks
                                toggle_deck=toggle_deck
                                untoggle_deck=untoggle_deck
                            />
                        }
                        .into_any();
                        let page_section = view! {
                            <PageAssignmentSection
                                card_id=id
                                page_ids=page_ids
                                signed_in=signed_in
                                pages=pages
                                toggle_page=toggle_page
                                untoggle_page=untoggle_page
                            />
                        }
                        .into_any();
                        let links_section = view! {
                            <CardLinksSection
                                card_id=id
                                linked_cards=detail.linked_cards.clone()
                                signed_in=signed_in
                                add_link=add_link
                                remove_link=remove_link
                            />
                        }
                        .into_any();
                        let delete_form = signed_in.then(|| {
                            view! { <DeleteCardSection card_id=id delete=delete /> }.into_any()
                        });
                        let sign_in_prompt = (!signed_in).then(|| {
                            view! { <p><A href="/login">"Sign in"</A>" to edit this card."</p> }
                                .into_any()
                        });

                        view! {
                            {preview}
                            {edit_form}
                            {dictionary_editor}
                            {deck_section}
                            {page_section}
                            {links_section}
                            {delete_form}
                            {sign_in_prompt}
                        }
                        .into_any()
                    }
                }
            })}
        </Transition>
    }
}

#[component]
fn DecksPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| current_user_name());
    let create = ServerAction::<CreateDeck>::new();
    let rename = ServerAction::<RenameDeck>::new();
    let delete = ServerAction::<DeleteDeck>::new();
    let tags = Resource::new(|| (), |()| list_tags());
    let decks = Resource::new(
        move || {
            (
                create.version().get(),
                rename.version().get(),
                delete.version().get(),
            )
        },
        |_| list_decks(),
    );

    view! {
        <h1>"Decks"</h1>
        <p class="page-intro">
            "Group cards for focused review — a deck's cards show up in Review and in the card list's deck filter."
        </p>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                let signed_in = auth.await.ok().flatten().is_some();
                let create_form = signed_in.then(|| {
                    view! {
                        <ActionForm action=create>
                            <label>"New deck name" <input type="text" name="name" /></label>
                            <label>
                                "Group by tag (optional)"
                                <select name="group_by_tag">
                                    <option value="">"Don't group — start empty"</option>
                                    <Transition fallback=|| ()>
                                        {move || Suspend::new(async move {
                                            tags.await.unwrap_or_default().into_iter().map(|tag| {
                                                view! { <option value=tag.clone()>{tag.clone()}</option> }
                                            }).collect_view()
                                        })}
                                    </Transition>
                                </select>
                            </label>
                            <button type="submit">"Create"</button>
                        </ActionForm>
                        {move || {
                            create.value().get().and_then(Result::err).map(|e| {
                                view! { <p>"Error: " {e.to_string()}</p> }
                            })
                        }}
                    }
                });
                match decks.await {
                    Ok(decks) if decks.is_empty() => {
                        view! { {create_form} <p>"No decks yet."</p> }.into_any()
                    }
                    Ok(decks) => view! {
                        {create_form}
                        <ul class="deck-cabinet">
                            {decks
                                .into_iter()
                                .map(|deck| {
                                    let href = format!("/?deck={}", deck.id);
                                    let deck_id = deck.id;
                                    let name_value = deck.name.clone();
                                    let peek_card = deck
                                        .sample_word
                                        .clone()
                                        .map(|word| view! { <div class="peek-card">{word}</div> });
                                    let manage = signed_in.then(|| {
                                        let delete_armed = RwSignal::new(false);
                                        view! {
                                            <div class="manage">
                                                <ActionForm action=rename>
                                                    <input type="hidden" name="deck_id" value=deck_id.to_string() />
                                                    <input type="text" name="name" value=name_value />
                                                    <button type="submit">"Rename"</button>
                                                </ActionForm>
                                                <ActionForm action=delete>
                                                    <input type="hidden" name="deck_id" value=deck_id.to_string() />
                                                    {confirm_button("Delete", delete_armed)}
                                                </ActionForm>
                                            </div>
                                        }
                                    });
                                    view! {
                                        <li>
                                            {peek_card}
                                            <div class="label-holder"><a href=href>{deck.name}</a></div>
                                            <div class="pull"></div>
                                            {manage}
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                        // rename/delete are one ServerAction shared across every
                        // row's form, so their feedback is page-level, not
                        // per-row — correct given today's layout, worth
                        // revisiting if Phase B gives each row its own action.
                        {move || {
                            rename.value().get().and_then(Result::err).map(|e| {
                                view! { <p>"Error renaming deck: " {e.to_string()}</p> }
                            })
                        }}
                        {move || {
                            delete.value().get().and_then(Result::err).map(|e| {
                                view! { <p>"Error deleting deck: " {e.to_string()}</p> }
                            })
                        }}
                    }
                    .into_any(),
                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                }
            })}
        </Transition>
    }
}

/// `pages` is the *whole* tree, re-filtered to `parent_id`'s children at
/// each level — O(pages²) in the worst case, trivial at this app's scale
/// (a handful of editors' worth of grammar articles, not thousands of
/// pages). `Arc`, not `&[...]`: each recursive call's `view!` closure needs
/// to own what it captures (views can outlive the function call that built
/// them), which a borrow can't satisfy — cloning the `Arc` is a refcount
/// bump, not a copy of the data. `Arc` rather than the cheaper `Rc`
/// specifically because `WikiPage` below holds this across an `.await`
/// inside `Suspend::new`, which requires the held future to be `Send` —
/// `Rc` isn't. A recursive fn returning `impl IntoView` doesn't compile
/// either way (the opaque return type can't reference itself), so this
/// returns `AnyView` instead — the one place in this file that needs to,
/// for exactly that reason. Taking `Arc<...>` by value rather than
/// `&Arc<...>` (clippy's usual preference) is deliberate too: every
/// recursive call needs its own owned clone to move into a `view!` closure,
/// so a borrowed parameter would just mean cloning it immediately anyway.
/// `adding_child_to`: at most one node in the whole tree has its inline
/// "add a child page" field open at a time — Confluence's sidebar "+" does
/// the same (one target, not a form per row all the time), and a single
/// shared signal is simpler than threading per-node state through the
/// recursion.
#[allow(clippy::needless_pass_by_value)]
/// Where a page being dragged in the sidebar tree would land if dropped
/// right now — `Before` a specific sibling, or `EndOf` a parent's children
/// (the synthetic drop zone after the last sibling, since "before row K"
/// alone can't express "after the last one"). Together these two cover
/// every possible insertion point among a set of siblings without needing
/// to split each row into a top/bottom half (which would need
/// `getBoundingClientRect()`, a `web_sys` call this file otherwise avoids
/// outside the one `#[cfg(feature = "hydrate")]`-gated spot it already
/// has, in `wiki_page_actions_menu` below.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DragTarget {
    Before(Uuid),
    EndOf(Option<Uuid>),
}

/// `wiki_tree_level`'s own drag-in-progress state — grouped the same way
/// `WikiPageActions` above groups `wiki_page_actions_menu`'s params, to
/// keep this (and the function's other params) under clippy's
/// `too_many_arguments`.
#[derive(Clone, Copy)]
struct TreeDragState {
    dragging_page: RwSignal<Option<Uuid>>,
    drag_over: RwSignal<Option<DragTarget>>,
}

// A dense, cohesive recursive view-builder — splitting it further would
// scatter state that's only meaningful read together (see TreeDragState's
// own doc comment on why that state is already grouped as far as it
// usefully can be).
#[allow(clippy::too_many_lines)]
fn wiki_tree_level(
    pages: &std::sync::Arc<Vec<crate::wiki::PageTreeNode>>,
    parent_id: Option<Uuid>,
    signed_in: bool,
    create: ServerAction<CreatePage>,
    reorder: ServerAction<ReorderPage>,
    adding_child_to: RwSignal<Option<Uuid>>,
    drag: TreeDragState,
) -> AnyView {
    let children: Vec<_> = pages.iter().filter(|p| p.parent_id == parent_id).collect();
    if children.is_empty() {
        return ().into_any();
    }
    // Scopes drag-and-drop to siblings only (same parent_id) — every row
    // built below shares that same parent, so this one list gates all of
    // them. Reparenting via drag isn't handled here; the "Move" picker
    // elsewhere still owns that.
    let sibling_ids: Vec<Uuid> = children.iter().map(|p| p.id).collect();
    let last_sibling_id = *sibling_ids
        .last()
        .expect("children is non-empty, checked above");
    let rows: Vec<_> = children
        .into_iter()
        .map(|page| {
            let page_id = page.id;
            let href = format!("/wiki/{}/{}", page.id, page.slug);
            let title = page.title.clone();
            let sub = wiki_tree_level(
                pages,
                Some(page_id),
                signed_in,
                create,
                reorder,
                adding_child_to,
                drag,
            );
            (page_id, href, title, sub)
        })
        .collect();
    view! {
        <ul class="wiki-tree">
            {rows
                .into_iter()
                .map(|(page_id, href, title, sub)| {
                    let new_child_title = RwSignal::new(String::new());
                    let siblings_for_dragover = sibling_ids.clone();
                    let siblings_for_drop = sibling_ids.clone();
                    view! {
                        <li>
                            <details open>
                                <summary
                                    draggable=signed_in.then_some("true")
                                    class:wiki-tree-dragging=move || {
                                        signed_in && drag.dragging_page.get() == Some(page_id)
                                    }
                                    class:wiki-tree-drag-over=move || {
                                        signed_in
                                            && drag.drag_over.get() == Some(DragTarget::Before(page_id))
                                    }
                                    on:dragstart=move |_| drag.dragging_page.set(Some(page_id))
                                    on:dragend=move |_| {
                                        drag.dragging_page.set(None);
                                        drag.drag_over.set(None);
                                    }
                                    on:dragover=move |ev| {
                                        let valid = drag.dragging_page.get().is_some_and(|dragged| {
                                            dragged != page_id
                                                && siblings_for_dragover.contains(&dragged)
                                        });
                                        if valid {
                                            ev.prevent_default();
                                            drag.drag_over.set(Some(DragTarget::Before(page_id)));
                                        }
                                    }
                                    on:drop=move |ev| {
                                        ev.prevent_default();
                                        if let Some(dragged) = drag.dragging_page.get()
                                            && dragged != page_id
                                            && siblings_for_drop.contains(&dragged)
                                        {
                                            reorder
                                                .dispatch(ReorderPage {
                                                    page_id: dragged,
                                                    relative_to: page_id,
                                                    insert_after: false,
                                                });
                                        }
                                        drag.dragging_page.set(None);
                                        drag.drag_over.set(None);
                                    }
                                >
                                    <A attr:draggable="false" href=href>{title}</A>
                                    {signed_in.then(|| view! {
                                        // prevent_default so this click adds a
                                        // child instead of toggling the
                                        // <details> open/closed — summary's
                                        // native behavior otherwise fires on
                                        // any click landing inside it.
                                        <button
                                            type="button"
                                            class="wiki-tree-add"
                                            title="Add a child page here"
                                            draggable="false"
                                            on:click=move |ev| {
                                                ev.prevent_default();
                                                adding_child_to.update(|cur| {
                                                    *cur = if *cur == Some(page_id) {
                                                        None
                                                    } else {
                                                        Some(page_id)
                                                    };
                                                });
                                            }
                                        >
                                            "+"
                                        </button>
                                    })}
                                </summary>
                                {move || {
                                    (adding_child_to.get() == Some(page_id))
                                        .then(|| view! {
                                            <form
                                                class="wiki-tree-add-form"
                                                on:submit=move |ev| {
                                                    ev.prevent_default();
                                                    let title = new_child_title.get();
                                                    if !title.trim().is_empty() {
                                                        create
                                                            .dispatch(CreatePage {
                                                                parent_id: Some(page_id),
                                                                title,
                                                            });
                                                        adding_child_to.set(None);
                                                    }
                                                }
                                            >
                                                <input
                                                    type="text"
                                                    placeholder="Page title"
                                                    required
                                                    prop:value=move || new_child_title.get()
                                                    on:input=move |ev| {
                                                        new_child_title.set(event_target_value(&ev));
                                                    }
                                                />
                                                <button type="submit">"Add"</button>
                                            </form>
                                        })
                                }}
                                {sub}
                            </details>
                        </li>
                    }
                })
                .collect_view()}
            {signed_in
                .then(|| {
                    let siblings = sibling_ids.clone();
                    let siblings_for_dragover = sibling_ids.clone();
                    let siblings_for_drop = sibling_ids.clone();
                    view! {
                        // "Before row K" alone can't express "after the very
                        // last sibling" — this is that missing drop target.
                        // Zero-height and invisible to the drop-detection
                        // logic unless a sibling of this exact list is
                        // actively being dragged, so it doesn't clutter the
                        // tree or intercept drops the rest of the time.
                        <li
                            class="wiki-tree-drop-end"
                            class:wiki-tree-drag-over=move || {
                                drag.drag_over.get() == Some(DragTarget::EndOf(parent_id))
                            }
                            style:display=move || {
                                let active = drag
                                    .dragging_page
                                    .get()
                                    .is_some_and(|dragged| siblings.contains(&dragged));
                                if active { "block" } else { "none" }
                            }
                            on:dragover=move |ev| {
                                if drag.dragging_page.get().is_some_and(|dragged| siblings_for_dragover.contains(&dragged))
                                {
                                    ev.prevent_default();
                                    drag.drag_over.set(Some(DragTarget::EndOf(parent_id)));
                                }
                            }
                            on:drop=move |ev| {
                                ev.prevent_default();
                                if let Some(dragged) = drag.dragging_page.get()
                                    && dragged != last_sibling_id
                                    && siblings_for_drop.contains(&dragged)
                                {
                                    reorder
                                        .dispatch(ReorderPage {
                                            page_id: dragged,
                                            relative_to: last_sibling_id,
                                            insert_after: true,
                                        });
                                }
                                drag.dragging_page.set(None);
                                drag.drag_over.set(None);
                            }
                        ></li>
                    }
                })}
        </ul>
    }
    .into_any()
}

/// The sidebar's own tree + "new top-level page" form, built from data the
/// caller already resolved (not a `#[component]`/its own `Resource` — both
/// `WikiListPage` and `WikiPage` already fetch the tree for their own
/// purposes, and `WikiPage` needs it for breadcrumbs too, so this avoids a
/// second independent fetch of the same data on every wiki page).
///
/// Returns `AnyView`, like `wiki_tree_level`/`wiki_search_results_view`
/// below it: `WikiPage` embeds this inline, and leaving it as a plain
/// `impl IntoView` let its whole nested search/`Transition`/tree type get
/// inlined into `WikiPage`'s own view type — confirmed as the actual
/// culprit (named directly in rustc's error) the one time bumping
/// `recursion_limit` alone wasn't enough to stop `cargo leptos build
/// --release` overflowing rustc's query-depth limit.
fn wiki_sidebar(
    pages: &std::sync::Arc<Vec<PageTreeNode>>,
    signed_in: bool,
    create: ServerAction<CreatePage>,
    reorder: ServerAction<ReorderPage>,
    new_title: RwSignal<String>,
) -> AnyView {
    let create_form = signed_in.then(|| {
        view! {
            <form on:submit=move |ev| {
                ev.prevent_default();
                let title = new_title.get();
                if !title.trim().is_empty() {
                    create.dispatch(CreatePage { parent_id: None, title });
                    new_title.set(String::new());
                }
            }>
                <label>
                    "New page"
                    <input
                        type="text"
                        placeholder="Page title"
                        required
                        prop:value=move || new_title.get()
                        on:input=move |ev| new_title.set(event_target_value(&ev))
                    />
                </label>
                <button type="submit">"Create"</button>
            </form>
        }
    });
    let adding_child_to = RwSignal::new(None::<Uuid>);
    let drag = TreeDragState {
        dragging_page: RwSignal::new(None::<Uuid>),
        drag_over: RwSignal::new(None::<DragTarget>),
    };
    let tree_view = if pages.is_empty() {
        view! { <p class="wiki-sidebar-empty">"No pages yet."</p> }.into_any()
    } else {
        wiki_tree_level(
            pages,
            None,
            signed_in,
            create,
            reorder,
            adding_child_to,
            drag,
        )
    };

    // Full-text search over `search_pages` (Postgres tsvector, already built
    // for this — just never wired to anything in the UI). Re-fetches on
    // every keystroke rather than debouncing: at this app's scale (a
    // handful of editors' worth of pages) that's not a real cost, and it
    // keeps this simple. The tree and the results list both stay mounted
    // (toggled with `style:display`, not conditionally rendered) because
    // `create_form`/`tree_view` aren't `Clone` and a reactive `{move || ..}`
    // block needs to rebuild its content on every call, which would require
    // cloning them.
    let search_query = RwSignal::new(String::new());
    let search_results = Resource::new(
        move || search_query.get(),
        |q| async move {
            let q = q.trim().to_string();
            if q.is_empty() {
                Ok(Vec::new())
            } else {
                search_pages(q).await
            }
        },
    );
    let searching = move || !search_query.get().trim().is_empty();

    view! {
        <aside class="wiki-sidebar">
            <A attr:class="wiki-sidebar-home" href="/wiki">"All pages"</A>
            <div class="wiki-search">
                <input
                    type="search"
                    class="wiki-search-input"
                    placeholder="Search pages…"
                    prop:value=move || search_query.get()
                    on:input=move |ev| search_query.set(event_target_value(&ev))
                />
            </div>
            <div
                class="wiki-search-results"
                style:display=move || if searching() { "block" } else { "none" }
            >
                <Transition fallback=|| view! { <p class="wiki-sidebar-empty">"Searching…"</p> }>
                    {move || Suspend::new(async move { wiki_search_results_view(search_results.await) })}
                </Transition>
            </div>
            <div
                class="wiki-tree-wrap"
                style:display=move || if searching() { "none" } else { "block" }
            >
                {create_form}
                {tree_view}
            </div>
        </aside>
    }
        .into_any()
}

/// Pulled out of `wiki_sidebar` purely to keep that function under
/// clippy's line-count limit — the match itself has no reason to live
/// inline in the `Suspend` closure.
fn wiki_search_results_view(
    results: Result<Vec<crate::wiki::PageSearchResult>, ServerFnError>,
) -> AnyView {
    match results {
        Err(e) => view! { <p class="wiki-sidebar-empty">"Error: " {e.to_string()}</p> }.into_any(),
        Ok(results) if results.is_empty() => {
            view! { <p class="wiki-sidebar-empty">"No matches."</p> }.into_any()
        }
        Ok(results) => view! {
            <ul class="wiki-search-result-list">
                {results
                    .into_iter()
                    .map(|r| {
                        let href = format!("/wiki/{}/{}", r.id, r.slug);
                        view! {
                            <li>
                                <A href=href>{r.title}</A>
                                <p class="wiki-search-snippet" inner_html=r.snippet></p>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        }
        .into_any(),
    }
}

/// Same ancestor walk as `wiki_breadcrumbs`, but as a plain "Parent ›
/// Child" string rather than a view — used where a page needs to be
/// identified in a flat list (the card-edit "Grammar pages" picker) rather
/// than rendered as its own page's nav trail.
fn page_path_label(pages: &[PageTreeNode], id: Uuid) -> String {
    let mut chain = Vec::new();
    let mut cursor = Some(id);
    while let Some(id) = cursor {
        let Some(node) = pages.iter().find(|p| p.id == id) else {
            break;
        };
        chain.push(node.title.clone());
        cursor = node.parent_id;
    }
    chain.reverse();
    chain.join(" › ")
}

/// Walks `current_id`'s `parent_id` chain back to a root, using the same
/// tree data the sidebar already has — one query answers both, rather than
/// a dedicated "ancestors of X" endpoint.
fn wiki_breadcrumbs(pages: &[PageTreeNode], current_id: Uuid) -> impl IntoView {
    let mut chain = Vec::new();
    let mut cursor = Some(current_id);
    while let Some(id) = cursor {
        let Some(node) = pages.iter().find(|p| p.id == id) else {
            break;
        };
        chain.push((node.id, node.title.clone(), node.slug.clone()));
        cursor = node.parent_id;
    }
    chain.reverse();
    let crumb_count = chain.len();
    view! {
        <nav class="wiki-breadcrumbs" aria-label="Breadcrumb">
            <A href="/wiki">"Grammar"</A>
            {chain
                .into_iter()
                .enumerate()
                .map(|(i, (id, title, slug))| {
                    let is_last = i + 1 == crumb_count;
                    view! {
                        <span class="wiki-breadcrumb-sep">"›"</span>
                        {if is_last {
                            view! { <span>{title}</span> }.into_any()
                        } else {
                            let href = format!("/wiki/{id}/{slug}");
                            view! { <A href=href>{title}</A> }.into_any()
                        }}
                    }
                })
                .collect_view()}
        </nav>
    }
}

/// `root` itself plus every page transitively parented under it — the set
/// the "Move" picker (`WikiPage`) must exclude from its target list, since
/// reparenting a page under itself or one of its own descendants would
/// put a cycle in the tree. Computed client-side from data already
/// fetched for the tree, rather than a second recursive query — the
/// server-side `move_page` trusts this list was built correctly and
/// doesn't re-derive/re-check it itself (see the doc comment on
/// `wiki::move_page`).
fn page_and_descendant_ids(pages: &[PageTreeNode], root: Uuid) -> std::collections::HashSet<Uuid> {
    let mut ids = std::collections::HashSet::new();
    ids.insert(root);
    // Fixed-point over parent_id links rather than a real tree walk —
    // `pages` is small (a handful of wiki pages, not thousands), so a
    // few extra passes over the whole list costs nothing and needs no
    // children-adjacency structure to be built first.
    loop {
        let before = ids.len();
        for p in pages {
            if let Some(parent) = p.parent_id
                && ids.contains(&parent)
            {
                ids.insert(p.id);
            }
        }
        if ids.len() == before {
            break;
        }
    }
    ids
}

/// The delete/move actions `wiki_page_actions_menu` dispatches, plus each
/// one's own transient UI state — grouped the same way `db.rs`'s
/// `WordFields` is, so this (and `wiki_page_actions_menu`'s other params)
/// doesn't trip clippy's `too_many_arguments`.
#[derive(Clone, Copy)]
struct WikiPageActions {
    delete: ServerAction<DeletePage>,
    delete_armed: RwSignal<bool>,
    move_page: ServerAction<MovePage>,
    move_open: RwSignal<bool>,
}

/// The "⋯" menu — copy link / toggle history / move / delete — plus its
/// own Move popover and move-dispatch error. Pulled out of `WikiPage`'s
/// view and `AnyView`-erased, the same reason and the same way as
/// `wiki_sidebar` above: this specific piece (named directly in rustc's
/// error) was still enough on its own to overflow `cargo leptos build
/// --release`'s query-depth limit even after other pieces nearby were
/// already erased.
fn wiki_page_actions_menu(
    id: Uuid,
    pages: &[PageTreeNode],
    actions: WikiPageActions,
    show_history: RwSignal<bool>,
    diff_target: RwSignal<Option<Uuid>>,
    link_copied: RwSignal<bool>,
) -> AnyView {
    let WikiPageActions {
        delete,
        delete_armed,
        move_page,
        move_open,
    } = actions;
    // Every page except this one and its own descendants — moving under
    // either would put a cycle in the tree.
    let excluded = page_and_descendant_ids(pages, id);
    let move_targets: Vec<(Uuid, String)> = pages
        .iter()
        .filter(|p| !excluded.contains(&p.id))
        .map(|p| (p.id, p.title.clone()))
        .collect();
    let more_menu_ref = NodeRef::<html::Details>::new();
    view! {
        <details class="wiki-more" node_ref=more_menu_ref>
            <summary>"⋯"</summary>
            <div class="wiki-more-items">
                <button
                    type="button"
                    on:click=move |_| {
                        #[cfg(feature = "hydrate")]
                        if let Some(window) = web_sys::window()
                            && let Ok(origin) = window.location().origin()
                        {
                            let url = format!("{origin}/wiki/{id}");
                            let _ = window.navigator().clipboard().write_text(&url);
                            link_copied.set(true);
                            set_timeout(
                                move || link_copied.set(false),
                                std::time::Duration::from_secs(2),
                            );
                        }
                    }
                >
                    {move || if link_copied.get() { "Copied!" } else { "Copy link" }}
                </button>
                <button
                    type="button"
                    on:click=move |_| {
                        show_history.update(|s| *s = !*s);
                        diff_target.set(None);
                    }
                >
                    {move || {
                        if show_history.get() {
                            "Hide history"
                        } else {
                            "History"
                        }
                    }}
                </button>
                <button
                    type="button"
                    on:click=move |_| {
                        move_open.update(|s| *s = !*s);
                        // The native <details> toggling
                        // itself open/closed and this
                        // button living inside it are
                        // independent — without this,
                        // the "..." dropdown stayed
                        // open underneath the Move
                        // popover (confirmed live).
                        if let Some(el) = more_menu_ref.get() {
                            el.set_open(false);
                        }
                    }
                >
                    "Move"
                </button>
                <ActionForm action=delete>
                    <input type="hidden" name="page_id" value=id.to_string() />
                    {confirm_button("Delete page", delete_armed)}
                </ActionForm>
            </div>
        </details>
        {wiki_move_popover(id, move_targets, move_page, move_open)}
    }
    .into_any()
}

/// The Move popover `wiki_page_actions_menu`'s own "Move" button opens,
/// plus its dispatch error. Pulled out mainly to keep that function under
/// clippy's line-count limit, same reason as `wiki_search_results_view`'s
/// own doc comment — not a query-depth fix in its own right the way
/// `wiki_page_actions_menu` itself is, but `AnyView`-erasing it costs
/// nothing and keeps the pattern consistent.
fn wiki_move_popover(
    id: Uuid,
    move_targets: Vec<(Uuid, String)>,
    move_page: ServerAction<MovePage>,
    move_open: RwSignal<bool>,
) -> impl Fn() -> Option<AnyView> {
    move || {
        let popover = move_open.get().then(|| {
            view! {
                <div class="wiki-move-popover">
                    <label>"Move to"</label>
                    <select on:change=move |ev| {
                        let value = event_target_value(&ev);
                        let new_parent_id = Uuid::parse_str(&value).ok();
                        move_page.dispatch(MovePage { page_id: id, new_parent_id });
                        move_open.set(false);
                    }>
                        <option value="" selected=true disabled=true>
                            "Choose a page…"
                        </option>
                        <option value="">"(top level)"</option>
                        {move_targets
                            .iter()
                            .map(|(target_id, title)| {
                                view! {
                                    <option value=target_id.to_string()>{title.clone()}</option>
                                }
                            })
                            .collect_view()}
                    </select>
                </div>
            }
        });
        let error = move_page.value().get().and_then(Result::err).map(|e| {
            view! { <p>"Error moving: " {e.to_string()}</p> }
        });
        Some(view! { {popover} {error} }.into_any())
    }
}

/// Delete/rename dispatch errors — block-level messages, kept separate
/// from `wiki_page_actions_menu`'s own button strip rather than squeezed
/// into it. `AnyView`-erased for the same query-depth reason as that
/// function.
fn wiki_page_action_errors(
    delete: ServerAction<DeletePage>,
    rename: ServerAction<RenamePage>,
) -> AnyView {
    view! {
        {move || {
            delete.value().get().and_then(Result::err).map(|e| {
                view! { <p>"Error deleting: " {e.to_string()}</p> }
            })
        }}
        {move || {
            rename.value().get().and_then(Result::err).map(|e| {
                view! { <p>"Error renaming: " {e.to_string()}</p> }
            })
        }}
    }
    .into_any()
}

/// The revision history panel — a `Transition`/`Suspend`'d list of past
/// revisions plus an inline diff viewer. Returns a closure rather than
/// calling into the view eagerly, since `WikiPage` embeds this reactively
/// (the same way it already embeds `save_status`): `show_history` toggles
/// it without the whole page re-resolving. `AnyView`-erased inside for the
/// same query-depth reason as `wiki_page_actions_menu`.
fn wiki_history_panel(
    id: Uuid,
    signed_in: bool,
    show_history: RwSignal<bool>,
    diff_target: RwSignal<Option<Uuid>>,
    restore: ServerAction<RestoreRevision>,
    revisions: Resource<Result<Option<Vec<PageRevisionSummary>>, ServerFnError>>,
    diff: Resource<Result<Option<RevisionDiff>, ServerFnError>>,
) -> impl Fn() -> Option<AnyView> {
    move || {
        show_history.get().then(|| {
            view! {
                <Transition fallback=|| view! { <p>"Loading…"</p> }>
                    {move || Suspend::new(async move {
                        match revisions.await {
                            Ok(Some(revs)) if revs.is_empty() => {
                                view! { <p>"No revisions yet."</p> }.into_any()
                            }
                            Ok(Some(revs)) => {
                                view! {
                                    <ul class="wiki-history-list">
                                        {revs
                                            .into_iter()
                                            .map(|r| {
                                                let rid = r.id;
                                                view! {
                                                    <li>
                                                        <span>{r.created_at}</span>
                                                        <button
                                                            type="button"
                                                            on:click=move |_| diff_target.set(Some(rid))
                                                        >
                                                            "View diff"
                                                        </button>
                                                        {signed_in.then(|| view! {
                                                            <button
                                                                type="button"
                                                                on:click=move |_| {
                                                                    restore
                                                                        .dispatch(RestoreRevision {
                                                                            page_id: id,
                                                                            revision_id: rid,
                                                                        });
                                                                    show_history.set(false);
                                                                }
                                                            >
                                                                "Restore"
                                                            </button>
                                                        })}
                                                    </li>
                                                }
                                            })
                                            .collect_view()}
                                    </ul>
                                    {move || Suspend::new(async move {
                                        diff.await
                                            .ok()
                                            .flatten()
                                            .map(|d| {
                                                view! {
                                                    <div class="wiki-diff" inner_html=d.diff_html></div>
                                                }
                                            })
                                    })}
                                }
                                    .into_any()
                            }
                            _ => ().into_any(),
                        }
                    })}
                </Transition>
            }
                .into_any()
        })
    }
}

#[component]
fn WikiListPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| current_user_name());
    let create = ServerAction::<CreatePage>::new();
    let reorder = ServerAction::<ReorderPage>::new();
    let new_title = RwSignal::new(String::new());
    let tree = Resource::new(
        move || (create.version().get(), reorder.version().get()),
        |_| list_page_tree(),
    );

    // Creating a page and then leaving the writer to go find it themselves
    // in the tree was the whole "terrible UX" complaint — jump straight
    // into it, same as Confluence/Docs do.
    let navigate = use_navigate();
    Effect::new(move |_| {
        if let Some(Ok(id)) = create.value().get() {
            navigate(
                &format!("/wiki/{id}"),
                leptos_router::NavigateOptions::default(),
            );
        }
    });

    view! {
        <h1>"Grammar"</h1>
        <p class="page-intro">
            "Articles about the language's grammar and structure — anyone signed in can write or reorganize a page."
        </p>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                let signed_in = auth.await.ok().flatten().is_some();
                match tree.await {
                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                    Ok(pages) => {
                        let sidebar = wiki_sidebar(
                            &std::sync::Arc::new(pages),
                            signed_in,
                            create,
                            reorder,
                            new_title,
                        );
                        view! {
                            <div class="wiki-shell">
                                {sidebar}
                                <div class="wiki-content">
                                    <p class="wiki-article-empty">
                                        "Pick a page from the list, or start a new one."
                                    </p>
                                </div>
                            </div>
                        }
                            .into_any()
                    }
                }
            })}
        </Transition>
    }
}

#[component]
fn WikiPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| current_user_name());
    let params = use_params_map();
    let page_id = move || {
        params
            .read()
            .get("id")
            .and_then(|id| Uuid::parse_str(&id).ok())
    };

    let save = ServerAction::<SavePageRevision>::new();
    let create = ServerAction::<CreatePage>::new();
    let rename = ServerAction::<RenamePage>::new();
    let delete = ServerAction::<DeletePage>::new();
    let restore = ServerAction::<RestoreRevision>::new();
    let move_page = ServerAction::<MovePage>::new();
    let reorder = ServerAction::<ReorderPage>::new();

    // Same as WikiListPage: jump straight into a newly created page (root
    // or child, via either "New page" form or the tree's inline "+")
    // instead of leaving the writer to go find it themselves.
    let navigate = use_navigate();
    Effect::new(move |_| {
        if let Some(Ok(id)) = create.value().get() {
            navigate(
                &format!("/wiki/{id}"),
                leptos_router::NavigateOptions::default(),
            );
        }
    });

    let delete_armed = RwSignal::new(false);
    let show_history = RwSignal::new(false);
    let move_open = RwSignal::new(false);
    let diff_target = RwSignal::new(None::<Uuid>);
    let new_root_title = RwSignal::new(String::new());
    // Confluence/Docs both default to a clean read view with an explicit
    // Edit button, not an always-live canvas — reverted back to that after
    // live feedback against real Confluence screenshots; see the "Edit"/
    // "Done" button below.
    let editing = RwSignal::new(false);
    // Set true for exactly one unmount by the "Discard" handler below, so
    // <WikiEditor>'s on_unmount_snapshot (see its own call site) knows to
    // skip its usual flush-save for this one exit — otherwise discarding
    // would just get silently overwritten by the very save it was trying
    // to avoid. Reset back to false by that same callback once read, not
    // left armed for the next, unrelated edit session on whatever page
    // comes next.
    let discard_requested = RwSignal::new(false);
    // Bumped exactly once, by the "Done" handler below, when leaving edit
    // mode — the one explicit signal that forces `page` to refetch (see its
    // own comment: it deliberately ignores save.version(), since refetching
    // mid-edit is what was destroying the live editor on every autosave
    // tick). Without this, the read view `page` feeds would keep showing
    // whatever was loaded before editing began, forever — confirmed live:
    // "/tab" and a columns edit both landed correctly in the database but
    // never appeared in the read view until a full page reload.
    let page_refresh = RwSignal::new(0u32);
    let link_copied = RwSignal::new(false);
    // Kept in sync with the server (see the Effect below) and read by the
    // autosave loop — plain signals, not part of the page Resource itself,
    // so the inline title edit and the editor's own autosave can both
    // update/read them without forcing a full page refetch on every click.
    let title_signal = RwSignal::new(String::new());
    // Compared against content_json, not content_markdown — the markdown
    // export is lossy (see render_blocks_html's doc comment in wiki.rs: it
    // drops text/highlight color outright and has no representation for
    // alignment at all), so a dirty-check against it treats a color- or
    // alignment-only edit as "nothing changed" and never saves it. Confirmed
    // live: applying color, then later alignment, to already-saved text —
    // no new typing either time — never dispatched a save at all, silently,
    // both times, with the exact right-looking editor state right up until
    // the page left edit mode and the change was simply gone.
    let last_saved_content_json = RwSignal::new(String::new());

    let tree = Resource::new(
        move || {
            (
                create.version().get(),
                rename.version().get(),
                delete.version().get(),
                move_page.version().get(),
                reorder.version().get(),
            )
        },
        |_| list_page_tree(),
    );
    // Not keyed on save.version() — confirmed live this was the actual
    // cause of a separate, worse bug than it looks like at a glance:
    // selecting text in the editor, then watching the highlight vanish a
    // few seconds later. `page` refetching rebuilds the <WikiEditor> view
    // below from scratch (new BlockNoteView, new BlockNote editor
    // instance), since it's rendered from this Resource's resolved value —
    // every autosave tick was round-tripping to the server and then tearing
    // down and remounting the live, currently-being-typed-into editor with
    // whatever it just fetched back, which blows away the DOM selection
    // (and, worse, would silently discard any keystrokes typed during that
    // round trip, landing after the fetch but before the remount replaced
    // the DOM under them). last_saved_content_json/title_signal don't need
    // this refetch to stay in sync either — the autosave loop and the
    // Done handler already set them directly, from the exact content_json
    // they just sent, right after each dispatch (see last_saved_content_json
    // below). rename/restore still belong here: both genuinely replace the
    // page's content/title from outside the live editor (restore loads a
    // past revision; the live editor has no idea), so re-fetching for those
    // is the correct behavior, not a bug. page_refresh is the Done handler's
    // own explicit, one-shot way to ask for the same refetch when leaving
    // edit mode — see its own comment.
    let page = Resource::new(
        move || {
            (
                page_id(),
                rename.version().get(),
                restore.version().get(),
                page_refresh.get(),
            )
        },
        |(id, ..)| async move {
            match id {
                Some(id) => get_page(id).await,
                None => Ok(None),
            }
        },
    );
    // The card side of the link (EditCardPage's "Grammar pages" checkboxes)
    // has no presence on this route to key a version signal off, so this
    // just re-fetches whenever the page itself changes — same as `page`.
    let linked_cards = Resource::new(page_id, |id| async move {
        match id {
            Some(id) => cards_for_page(id).await,
            None => Ok(Vec::new()),
        }
    });
    let revisions = Resource::new(
        move || {
            (
                page_id(),
                show_history.get(),
                save.version().get(),
                restore.version().get(),
            )
        },
        |(id, show, ..)| async move {
            match (id, show) {
                (Some(id), true) => list_page_revisions(id).await.map(Some),
                _ => Ok(None),
            }
        },
    );
    let diff = Resource::new(
        move || (page_id(), diff_target.get()),
        |(page_id, revision_id)| async move {
            match (page_id, revision_id) {
                (Some(page_id), Some(revision_id)) => {
                    get_revision_diff(page_id, revision_id).await.map(Some)
                }
                _ => Ok(None),
            }
        },
    );

    // Keeps title_signal/last_saved_content_json in step with the server's
    // current truth — fires on first load and again after a rename/restore
    // (since `page` itself refetches on those; a save keeps them in sync
    // directly instead, see the autosave loop and the Done handler below —
    // `page` deliberately doesn't refetch on save, see its own comment).
    // Reading `page.get()` here (not awaiting it) is what keeps this a
    // plain, immediately-rerunning Effect rather than another async
    // Suspend block.
    Effect::new(move |_| {
        if let Some(Ok(Some(p))) = page.get() {
            title_signal.set(p.title);
            last_saved_content_json.set(p.content_json);
        }
    });

    // Autosave — spawned once per distinct page_id (this Effect's closure
    // reads nothing else reactively, so it only reruns on navigation, not
    // on every save it itself triggers). Confluence's own answer to "don't
    // lose work": content saves itself periodically while you're writing,
    // full stop, no button to remember to click. The real gap against
    // Confluence specifically — a save here still lands in page_revisions
    // directly rather than a separate draft, so frequent edits do mean more
    // history entries than a deliberate "Publish" model would — is a
    // tradeoff accepted for now rather than adding draft storage on top of
    // everything else in flight; noted as the honest follow-up.
    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        let Some(id) = page_id() else { return };
        let mount_id = crate::wiki_editor::wiki_editor_mount_id(id);
        // Cancelled via on_cleanup, not by re-reading page_id() — this loop
        // outlives the Effect run that spawned it (it's a detached
        // wasm_bindgen_futures task, not something Leptos's own reactive
        // cleanup can reach into mid-sleep), so if WikiPage itself gets
        // disposed entirely (navigating to a non-wiki route) while this is
        // sleeping, page_id() would try to read params — owned by that now-
        // disposed scope — and panic. Confirmed live: navigating away from
        // a wiki page while its autosave loop was mid-wait reliably crashed
        // the whole app's reactive runtime this way. on_cleanup fires on
        // both a route-triggered rerun of this Effect (a new page_id) and
        // on WikiPage's own disposal, so one flag covers both cases this
        // loop needs to stop for.
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let cancelled = cancelled.clone();
            // on_cleanup requires Send + Sync (it's not wasm-specific) even
            // though this whole app is single-threaded — an AtomicBool is
            // the plain-std type satisfying that, not a sign of any real
            // cross-thread sharing here.
            on_cleanup(move || cancelled.store(true, std::sync::atomic::Ordering::Relaxed));
        }
        wasm_bindgen_futures::spawn_local(async move {
            loop {
                crate::wiki_editor::sleep_ms(6000).await;
                if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                // Errs here just mean no editor is mounted (signed-out, or
                // not loaded yet) — nothing to autosave, keep waiting.
                let Ok(content_json) = crate::wiki_editor::get_document(&mount_id).await else {
                    continue;
                };
                if content_json == last_saved_content_json.get_untracked() {
                    continue;
                }
                let Ok(content_markdown) = crate::wiki_editor::get_markdown(&mount_id).await else {
                    continue;
                };
                save.dispatch(SavePageRevision {
                    page_id: id,
                    title: title_signal.get_untracked(),
                    content_json: content_json.clone(),
                    content_markdown,
                });
                last_saved_content_json.set(content_json);
            }
        });
    });

    view! {
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || {
                // Read *before* Suspend's async block, same reason as the
                // nav's current-page highlight: a plain signal read after
                // an .await point inside Suspend doesn't reliably retrigger
                // this closure, so toggling Edit/Done would never actually
                // re-render the page.
                let is_editing = editing.get();
                Suspend::new(async move {
                let signed_in = auth.await.ok().flatten().is_some();
                let pages = tree.await.unwrap_or_default();
                let pages = std::sync::Arc::new(pages);
                let sidebar =
                    wiki_sidebar(&pages, signed_in, create, reorder, new_root_title);

                let body = match page.await {
                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                    // A bare, unstyled sentence floating next to the sidebar
                    // with no way back was a real dead end — most often hit
                    // by a stale link to a page someone else just deleted.
                    // Same card chrome as every other state here, plus the
                    // one thing actually useful to do next.
                    Ok(None) => view! {
                        <div class="wiki-article">
                            <p>"This page doesn't exist — it may have been deleted."</p>
                            <p><A href="/wiki">"← All pages"</A></p>
                        </div>
                    }
                    .into_any(),
                    Ok(Some(p)) => {
                        let id = p.id;
                        let content_json = p.content_json.clone();
                        let content_html = p.content_html.clone();
                        let is_empty = p.content_markdown.trim().is_empty();
                        let breadcrumbs = wiki_breadcrumbs(&pages, id);

                        // The title *is* the heading — Confluence and Docs
                        // both let you rename a page by just editing its
                        // title in place, not through a separate form. Only
                        // commits (dispatches RenamePage) on blur/Enter, and
                        // only if it actually changed.
                        let commit_title = move || {
                            let value = title_signal.get_untracked();
                            if !value.trim().is_empty() {
                                rename.dispatch(RenamePage { page_id: id, title: value });
                            }
                        };
                        let title_view = if signed_in && is_editing {
                            view! {
                                <input
                                    class="wiki-title-input"
                                    type="text"
                                    prop:value=move || title_signal.get()
                                    on:input=move |ev| title_signal.set(event_target_value(&ev))
                                    on:blur=move |_| commit_title()
                                    on:keydown=move |ev| {
                                        if ev.key() == "Enter" {
                                            ev.prevent_default();
                                            commit_title();
                                        }
                                    }
                                />
                            }
                                .into_any()
                        } else {
                            view! { <h1>{move || title_signal.get()}</h1> }.into_any()
                        };

                        let save_status = move || {
                            if save.pending().get() {
                                Some("Saving…")
                            } else if save.value().get().is_some_and(|r| r.is_ok()) {
                                Some("Saved")
                            } else {
                                None
                            }
                        };

                        // "Done" flushes one last save before leaving edit
                        // mode — the autosave loop runs every 6s, so without
                        // this, clicking Done right after typing could drop
                        // up to 6 seconds of work. Awaited directly via
                        // save_page_revision rather than save.dispatch(): the
                        // read view editing.set(false) is about to reveal
                        // needs this exact save to have actually landed, not
                        // just been fired off — dispatch() returns before
                        // the request completes, which would race
                        // page_refresh below. page_refresh bumps
                        // unconditionally, not only when this branch has
                        // something to flush, because an earlier autosave
                        // tick during this same edit session may already
                        // have saved content that `page` was never asked to
                        // refetch for either (autosave deliberately doesn't
                        // bump page_refresh — see its own comment). This is
                        // the one point refetching is safe: the editor is
                        // about to unmount, so there's no live selection
                        // left to lose. Confirmed live this was a real bug,
                        // not hypothetical: a columns edit and a text
                        // deletion both landed correctly in the database but
                        // never appeared in the read view.
                        let edit_toggle = signed_in.then(|| {
                            if is_editing {
                                view! {
                                    // No "are you sure" step, unlike delete (delete_armed)
                                    // below — the worst a discard can lose is whatever was
                                    // typed since the last autosave tick (at most 6s, see the
                                    // autosave loop above), and the page's full revision
                                    // history already covers wanting an older version back.
                                    // <WikiEditor>'s on_unmount_snapshot is what actually
                                    // skips the flush-save for this exit (see
                                    // discard_requested's own comment) — this handler's only
                                    // job is to arm that flag before leaving edit mode.
                                    <button
                                        type="button"
                                        class="wiki-edit-discard"
                                        on:click=move |_| {
                                            discard_requested.set(true);
                                            editing.set(false);
                                        }
                                    >
                                        "Discard"
                                    </button>
                                    <button
                                        type="button"
                                        class="wiki-edit-toggle"
                                        on:click=move |_| {
                                            #[cfg(feature = "hydrate")]
                                            {
                                                let mount_id = crate::wiki_editor::wiki_editor_mount_id(id);
                                                wasm_bindgen_futures::spawn_local(async move {
                                                    if let Ok(content_json)
                                                        = crate::wiki_editor::get_document(&mount_id).await
                                                        && content_json != last_saved_content_json.get_untracked()
                                                        && let Ok(content_markdown)
                                                            = crate::wiki_editor::get_markdown(&mount_id).await
                                                        && save_page_revision(
                                                            id,
                                                            title_signal.get_untracked(),
                                                            content_json.clone(),
                                                            content_markdown,
                                                        )
                                                        .await
                                                        .is_ok()
                                                    {
                                                        last_saved_content_json.set(content_json);
                                                    }
                                                    page_refresh.update(|n| *n += 1);
                                                    editing.set(false);
                                                });
                                            }
                                            #[cfg(not(feature = "hydrate"))]
                                            editing.set(false);
                                        }
                                    >
                                        "Done"
                                    </button>
                                }.into_any()
                            } else {
                                view! {
                                    <button
                                        type="button"
                                        class="wiki-edit-toggle"
                                        on:click=move |_| editing.set(true)
                                    >
                                        "Edit"
                                    </button>
                                }.into_any()
                            }
                        });

                        let secondary_actions = signed_in.then(|| {
                            wiki_page_actions_menu(
                                id,
                                &pages,
                                WikiPageActions {
                                    delete,
                                    delete_armed,
                                    move_page,
                                    move_open,
                                },
                                show_history,
                                diff_target,
                                link_copied,
                            )
                        });

                        // Separate from the menu itself — these are block-
                        // level messages, not something that belongs
                        // squeezed into the title row's button strip.
                        let action_errors = signed_in.then(|| wiki_page_action_errors(delete, rename));

                        let history_panel = wiki_history_panel(
                            id,
                            signed_in,
                            show_history,
                            diff_target,
                            restore,
                            revisions,
                            diff,
                        );

                        view! {
                            {breadcrumbs}
                            <div class="wiki-title-row">
                                {title_view}
                                <div class="wiki-title-row-actions">
                                    {move || {
                                        save_status()
                                            .map(|s| view! { <span class="wiki-save-status">{s}</span> })
                                    }}
                                    {edit_toggle}
                                    {secondary_actions}
                                </div>
                            </div>
                            {action_errors}
                            {history_panel}
                            {
                                if signed_in && is_editing {
                                    view! {
                                        // wiki-editor.js/.css are injected by WikiEditor itself
                                        // (see wiki_editor.rs) rather than declared here via
                                        // leptos_meta — confirmed live that a <Script>/
                                        // <Stylesheet> declared outside a stable top-level
                                        // position never actually lands in the DOM. Only
                                        // mounted while actually editing — a page that's only
                                        // ever read never ships BlockNote/React at all.
                                        <WikiEditor
                                            page_id=id
                                            initial_document_json=content_json.clone()
                                            read_only=false
                                            // Covers every way this editor can unmount without
                                            // going through the explicit "Done" button above —
                                            // right now, specifically, navigating to a different
                                            // wiki page mid-edit: `editing` is one signal shared
                                            // across every /wiki/:id route (this whole component
                                            // is reused across them, not remounted per page), so
                                            // without resetting it here too, the next page you
                                            // land on rendered straight into edit mode instead of
                                            // its read view — reported live as "one continuous
                                            // editing session" across unrelated pages. The dirty-
                                            // check mirrors "Done"'s own (content_json against
                                            // last_saved_content_json); id/title_signal are safe
                                            // to read here (not stale) because WikiEditor's own
                                            // on_cleanup captures its snapshot synchronously,
                                            // before `page_id`/`title_signal` have had any chance
                                            // to move on to a different page — see its own
                                            // comment for why that ordering matters.
                                            on_unmount_snapshot=move |content_json: String, content_markdown: String| {
                                                // discard_requested, not a plain bool check —
                                                // this callback is the one place that would
                                                // otherwise silently re-save a discarded edit
                                                // out from under the "Discard" button above
                                                // (its own click handler can't just skip its own
                                                // save: it never had one, the save only ever
                                                // happens here, on unmount — see this prop's own
                                                // doc comment).
                                                let discarding = discard_requested.get_untracked();
                                                if discarding {
                                                    discard_requested.set(false);
                                                }
                                                #[cfg(feature = "hydrate")]
                                                {
                                                    if !discarding && content_json != last_saved_content_json.get_untracked() {
                                                        let title = title_signal.get_untracked();
                                                        wasm_bindgen_futures::spawn_local(async move {
                                                            let _ = save_page_revision(
                                                                id,
                                                                title,
                                                                content_json,
                                                                content_markdown,
                                                            )
                                                            .await;
                                                        });
                                                    }
                                                }
                                                #[cfg(not(feature = "hydrate"))]
                                                {
                                                    let _ = (&content_json, &content_markdown, discarding);
                                                }
                                                editing.set(false);
                                            }
                                        />
                                    }
                                        .into_any()
                                } else if is_empty {
                                    view! {
                                        <p class="wiki-article-empty">"This page is empty."</p>
                                    }
                                        .into_any()
                                } else {
                                    view! { <div class="wiki-article" inner_html=content_html.clone()></div> }
                                        .into_any()
                                }
                            }
                            // The card side of this link (EditCardPage's "Grammar
                            // pages" checkboxes) is the only place to *create* it —
                            // this is purely a read-only backlink list, so the
                            // connection a card makes to this page is visible from
                            // here too, not just from the card's own edit form.
                            <Transition fallback=|| ()>
                                {move || Suspend::new(async move {
                                    match linked_cards.await {
                                        Ok(cards) if !cards.is_empty() => {
                                            view! {
                                                <div class="card-section">
                                                    <p class="card-section-heading">"Cards using this page"</p>
                                                    <ul class="ledger">
                                                        {cards.into_iter().map(|c| {
                                                            let href = format!("/cards/{}", c.id);
                                                            view! { <li><A href=href>{c.target_text}</A></li> }
                                                        }).collect_view()}
                                                    </ul>
                                                </div>
                                            }.into_any()
                                        }
                                        _ => ().into_any(),
                                    }
                                })}
                            </Transition>
                        }
                            .into_any()
                    }
                };

                view! {
                    <div class="wiki-shell">
                        {sidebar}
                        <div class="wiki-content">{body}</div>
                    </div>
                }
                })
            }}
        </Transition>
    }
}

#[component]
fn ReviewPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| current_user_name());
    let order = RwSignal::new(ReviewOrderInput::DueDate);
    let deck_filter = RwSignal::new(None::<Uuid>);
    let tag_filter = RwSignal::new(None::<String>);
    let decks = Resource::new(|| (), |()| list_decks());
    let tags = Resource::new(|| (), |()| list_tags());
    let grade = ServerAction::<GradeCard>::new();

    let queue = Resource::new(
        move || {
            (
                deck_filter.get(),
                tag_filter.get(),
                order.get(),
                grade.version().get(),
            )
        },
        |(deck_id, tag, order, _)| review_queue(deck_id, tag, order),
    );

    view! {
        <h1>"Review"</h1>
        <div>
            <label>
                "Filter by deck "
                <select on:change=move |ev| {
                    let value = event_target_value(&ev);
                    deck_filter.set(Uuid::parse_str(&value).ok());
                }>
                    <option value="">"All decks"</option>
                    <Transition fallback=|| ()>
                        {move || Suspend::new(async move {
                            decks.await.unwrap_or_default().into_iter().map(|deck| {
                                view! { <option value=deck.id.to_string()>{deck.name}</option> }
                            }).collect_view()
                        })}
                    </Transition>
                </select>
            </label>
            " "
            <label>
                "Filter by tag "
                <select on:change=move |ev| {
                    let value = event_target_value(&ev);
                    tag_filter.set(if value.is_empty() { None } else { Some(value) });
                }>
                    <option value="">"All tags"</option>
                    <Transition fallback=|| ()>
                        {move || Suspend::new(async move {
                            tags.await.unwrap_or_default().into_iter().map(|tag| {
                                view! { <option value=tag.clone()>{tag.clone()}</option> }
                            }).collect_view()
                        })}
                    </Transition>
                </select>
            </label>
            " "
            <label>
                "Order "
                <select on:change=move |ev| {
                    order.set(match event_target_value(&ev).as_str() {
                        "random" => ReviewOrderInput::Random,
                        "hardest" => ReviewOrderInput::HardestFirst,
                        _ => ReviewOrderInput::DueDate,
                    });
                }>
                    <option value="due">"Due date"</option>
                    <option value="random">"Random"</option>
                    <option value="hardest">"Hardest first"</option>
                </select>
            </label>
        </div>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                if auth.await.ok().flatten().is_none() {
                    return view! { <Redirect path="/login"/> }.into_any();
                }
                match queue.await {
                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                    Ok(q) => {
                        let counts = view! {
                            <p>
                                {q.due_review_count} " due · " {q.new_available_today} " new available today"
                            </p>
                        };
                        match q.cards.into_iter().next() {
                            None => view! { {counts} <p>"Nothing due. Nice."</p> }.into_any(),
                            Some(card) => {
                                let card_id = card.id;
                                let direction = card.direction;
                                // image_to_audio is the one type where the
                                // generic media_view below would be wrong, not
                                // just incomplete: it renders every set media
                                // field together in whichever slot (prompt or
                                // reveal) gets media_view whole, but here image
                                // is specifically the prompt and audio is
                                // specifically the (otherwise hidden) answer —
                                // showing both together up front would leak the
                                // answer before "Reveal". sentence_translation
                                // has no media at all (source_text fills the
                                // same slot media_view would), which the
                                // generic path already does handle inline below.
                                let prompt_and_answer = if card.card_type == "image_to_audio" {
                                    let image = card.image_url.map(|url| view! { <img src=url alt="" /> });
                                    let audio = card
                                        .audio_url
                                        .map(|url| view! { <audio controls src=url></audio> });
                                    match direction {
                                        DirectionInput::Forward => view! {
                                            {image}
                                            <details>
                                                <summary>"Reveal"</summary>
                                                {audio}
                                                <div class="word-display"><ClickableWord text=card.target_text /></div>
                                            </details>
                                        }
                                        .into_any(),
                                        DirectionInput::Reverse => view! {
                                            <div class="word-display"><ClickableWord text=card.target_text /></div>
                                            <details>
                                                <summary>"Reveal"</summary>
                                                {image}
                                                {audio}
                                            </details>
                                        }
                                        .into_any(),
                                    }
                                } else {
                                    let media_view = if card.card_type == "sentence_translation" {
                                        view! { <p>{card.source_text.unwrap_or_default()}</p> }
                                            .into_any()
                                    } else {
                                        view! {
                                            {card.image_url.map(|url| view! { <img src=url alt="" /> })}
                                            {card.audio_url.map(|url| view! { <audio controls src=url></audio> })}
                                            {card.video_url.map(|url| view! { <video controls src=url></video> })}
                                            {card.youtube_url.map(youtube_embed)}
                                        }
                                        .into_any()
                                    };
                                    // Forward: media is the prompt, target_text
                                    // the answer. Reverse (two-way cards only):
                                    // the same card, but target_text is the
                                    // prompt and media is the answer — this slot
                                    // is independently scheduled from the
                                    // forward one.
                                    match direction {
                                        DirectionInput::Forward => view! {
                                            {media_view}
                                            <details>
                                                <summary>"Reveal"</summary>
                                                <div class="word-display"><ClickableWord text=card.target_text /></div>
                                            </details>
                                        }
                                        .into_any(),
                                        DirectionInput::Reverse => view! {
                                            <div class="word-display"><ClickableWord text=card.target_text /></div>
                                            <details>
                                                <summary>"Reveal"</summary>
                                                {media_view}
                                            </details>
                                        }
                                        .into_any(),
                                    }
                                };
                                view! {
                                    {counts}
                                    <div class="review-card">
                                        {prompt_and_answer}
                                        <div class="review-grades">
                                            <button on:click=move |_| {
                                                grade.dispatch(GradeCard { card_id, grade: GradeInput::Again, direction });
                                            }>"Again"</button>
                                            <button on:click=move |_| {
                                                grade.dispatch(GradeCard { card_id, grade: GradeInput::Hard, direction });
                                            }>"Hard"</button>
                                            <button on:click=move |_| {
                                                grade.dispatch(GradeCard { card_id, grade: GradeInput::Good, direction });
                                            }>"Good"</button>
                                            <button on:click=move |_| {
                                                grade.dispatch(GradeCard { card_id, grade: GradeInput::Easy, direction });
                                            }>"Easy"</button>
                                        </div>
                                    </div>
                                }
                                .into_any()
                            }
                        }
                    }
                }
            })}
        </Transition>
    }
}

#[component]
fn StatsPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| current_user_name());
    let stats = Resource::new(|| (), |()| user_stats());

    view! {
        <h1>"Stats"</h1>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                if auth.await.ok().flatten().is_none() {
                    return view! { <Redirect path="/login"/> }.into_any();
                }
                match stats.await {
                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                    Ok(s) => view! {
                        <ul class="ledger">
                            <li>{s.total_cards} " cards in the shared pool"</li>
                            <li>{s.due_now} " due right now"</li>
                            <li>{s.reviews_today} " reviews logged today"</li>
                            <li>{s.current_streak_days} " day streak"</li>
                        </ul>
                        <h2>"Cards you find hard"</h2>
                        {if s.hardest_cards.is_empty() {
                            view! { <p>"None yet — nice."</p> }.into_any()
                        } else {
                            view! {
                                <ul class="ledger">
                                    {s.hardest_cards
                                        .into_iter()
                                        .map(|c| {
                                            let href = format!("/cards/{}", c.id);
                                            view! {
                                                <li>
                                                    <A href=href>
                                                        {c.target_text} " — " {c.lapses} " lapses, difficulty "
                                                        {format!("{:.2}", c.difficulty)}
                                                    </A>
                                                </li>
                                            }
                                        })
                                        .collect_view()}
                                </ul>
                            }
                            .into_any()
                        }}
                    }
                    .into_any(),
                }
            })}
        </Transition>
    }
}

#[component]
fn SettingsPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| current_user_name());
    let set_limit = ServerAction::<SetDailyNewCardLimit>::new();
    let limit = Resource::new(
        move || set_limit.version().get(),
        |_| crate::account::get_daily_new_card_limit(),
    );

    view! {
        <h1>"Settings"</h1>
        <p class="page-intro">"Tune how your own review sessions pace themselves."</p>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                if auth.await.ok().flatten().is_none() {
                    return view! { <Redirect path="/login"/> }.into_any();
                }
                let current = limit.await.ok().flatten();
                view! {
                    <ActionForm action=set_limit>
                        <label>
                            "New cards per day (blank = unlimited)"
                            <input
                                type="number"
                                name="limit"
                                min="0"
                                value=current.map(|n| n.to_string()).unwrap_or_default()
                            />
                        </label>
                        <button type="submit">"Save"</button>
                    </ActionForm>
                    {move || {
                        set_limit.value().get().map(|result| match result {
                            Ok(()) => view! { <p>"Saved."</p> }.into_any(),
                            Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                        })
                    }}
                }
                .into_any()
            })}
        </Transition>
    }
}

#[component]
fn AccountPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| current_user_name());
    let update_name = ServerAction::<UpdateName>::new();
    let update_email = ServerAction::<UpdateEmail>::new();
    let change_password = ServerAction::<ChangePassword>::new();
    let create_invite = ServerAction::<CreateInvite>::new();
    // Re-fetched after either name or email actually saves, so the forms
    // below reflect the new value rather than the stale one they were
    // first loaded with.
    let profile = Resource::new(
        move || (update_name.version().get(), update_email.version().get()),
        |_| current_profile(),
    );

    // Changing the password revokes every session server-side (see
    // account::change_password), so the only sane place left to land is
    // the login page — same message LoginPage already shows after a
    // password-reset redemption.
    Effect::new(move |_| {
        if matches!(change_password.value().get(), Some(Ok(()))) {
            reload_to("/login?reset=success");
        }
    });

    view! {
        <h1>"Account"</h1>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                if auth.await.ok().flatten().is_none() {
                    return view! { <Redirect path="/login"/> }.into_any();
                }
                let current = profile.await.ok();
                let name_value = current.as_ref().map(|p| p.name.clone()).unwrap_or_default();
                let email_value = current.as_ref().map(|p| p.email.clone()).unwrap_or_default();

                view! {
                    <h2>"Name"</h2>
                    <ActionForm action=update_name>
                        <label>"Name" <input type="text" name="name" value=name_value /></label>
                        <button type="submit">"Save"</button>
                    </ActionForm>
                    {move || {
                        update_name.value().get().map(|result| match result {
                            Ok(()) => view! { <p>"Saved."</p> }.into_any(),
                            Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                        })
                    }}

                    <h2>"Email"</h2>
                    <ActionForm action=update_email>
                        <label>
                            "New email"
                            <input type="email" name="new_email" value=email_value />
                        </label>
                        <label>
                            "Current password"
                            <input type="password" name="current_password" />
                        </label>
                        <button type="submit">"Save"</button>
                    </ActionForm>
                    {move || {
                        update_email.value().get().map(|result| match result {
                            Ok(()) => view! { <p>"Saved. Sign in with your new email next time."</p> }.into_any(),
                            Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                        })
                    }}

                    <h2>"Password"</h2>
                    <ActionForm action=change_password>
                        <label>
                            "Current password"
                            <input type="password" name="current_password" />
                        </label>
                        <label>"New password" <input type="password" name="new_password" /></label>
                        <label>
                            "Confirm new password"
                            <input type="password" name="confirm_password" />
                        </label>
                        <button type="submit">"Change password"</button>
                    </ActionForm>
                    {move || {
                        change_password.value().get().and_then(Result::err).map(|e| {
                            view! { <p>"Error: " {e.to_string()}</p> }
                        })
                    }}

                    <h2>"Invite a friend"</h2>
                    <p>
                        "Signup is invite-only. Any signed-in user can send an invite — "
                        "it emails them a signup link that expires in 7 days."
                    </p>
                    <ActionForm action=create_invite>
                        <label>"Their email" <input type="email" name="email" /></label>
                        <button type="submit">"Send invite"</button>
                    </ActionForm>
                    {move || {
                        create_invite.value().get().map(|result| match result {
                            Ok(()) => view! { <p>"Invite sent."</p> }.into_any(),
                            Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                        })
                    }}
                }
                .into_any()
            })}
        </Transition>
    }
}

#[cfg(test)]
mod tests {
    use super::parse_bulk_line;

    #[test]
    fn parses_a_plain_line() {
        assert_eq!(
            parse_bulk_line("dog,perro"),
            Some(("dog".to_string(), "perro".to_string()))
        );
    }

    #[test]
    fn trims_whitespace_on_both_sides() {
        assert_eq!(
            parse_bulk_line("  dog , perro  "),
            Some(("dog".to_string(), "perro".to_string()))
        );
    }

    #[test]
    fn keeps_a_second_comma_as_part_of_the_target_phrase() {
        assert_eq!(
            parse_bulk_line("hello, how are you,hola, ¿cómo estás?"),
            Some((
                "hello".to_string(),
                "how are you,hola, ¿cómo estás?".to_string()
            ))
        );
    }

    #[test]
    fn rejects_a_line_with_no_comma() {
        assert_eq!(parse_bulk_line("just one word"), None);
    }

    #[test]
    fn rejects_a_line_with_an_empty_side() {
        assert_eq!(parse_bulk_line(",perro"), None);
        assert_eq!(parse_bulk_line("dog,"), None);
        assert_eq!(parse_bulk_line(" , "), None);
    }

    #[test]
    fn rejects_an_empty_line() {
        assert_eq!(parse_bulk_line(""), None);
    }
}
