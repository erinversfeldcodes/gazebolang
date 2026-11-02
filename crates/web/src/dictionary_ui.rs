use crate::dictionary::{GetWordDefinition, LinkedWord, SaveWordDefinition, WordExample};
use leptos::html;
use leptos::prelude::*;

#[cfg(feature = "hydrate")]
use wasm_bindgen::JsCast;

/// Looks up `word`'s definition and opens the dialog — the single entry
/// point every trigger (the opener span, a related-word link, "Back")
/// dispatches through. Only the actual `show_modal` call is feature-gated,
/// since `web-sys`'s dialog methods only exist in the `hydrate` build (see
/// `media_recorder.rs`'s `RecordButton` for the same pattern and why: two
/// different view shapes per build has crashed hydration outright before
/// — this function's shape, not the view tree, is what differs here).
fn open_word_dialog(
    dialog_ref: NodeRef<html::Dialog>,
    lookup: ServerAction<GetWordDefinition>,
    current_word: RwSignal<String>,
    word: String,
) {
    current_word.set(word.clone());
    lookup.dispatch(GetWordDefinition { target_text: word });
    #[cfg(feature = "hydrate")]
    {
        if let Some(dialog) = dialog_ref.get_untracked() {
            let _ = dialog.show_modal();
        }
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = dialog_ref;
    }
}

fn close_word_dialog(dialog_ref: NodeRef<html::Dialog>) {
    #[cfg(feature = "hydrate")]
    {
        if let Some(dialog) = dialog_ref.get_untracked() {
            dialog.close();
        }
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = dialog_ref;
    }
}

/// `<dialog>` doesn't close on a backdrop click by default — this checks
/// whether the click landed on the dialog element itself (the backdrop)
/// rather than on any of its content, closing only in that case.
fn on_backdrop_click(ev: &leptos::ev::MouseEvent, dialog_ref: NodeRef<html::Dialog>) {
    #[cfg(feature = "hydrate")]
    {
        let clicked_the_dialog_itself = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlDialogElement>().ok())
            .zip(dialog_ref.get_untracked())
            .is_some_and(|(target, dialog)| target == dialog);
        if clicked_the_dialog_itself {
            close_word_dialog(dialog_ref);
        }
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (ev, dialog_ref);
    }
}

fn render_examples(examples: Vec<WordExample>) -> Option<impl IntoView> {
    (!examples.is_empty()).then(|| {
        view! {
            <div class="word-examples">
                <h4>"Examples"</h4>
                <ul>
                    {examples.into_iter().map(|ex| view! {
                        <li>
                            <p>{ex.target_sentence}</p>
                            {ex.gloss_sentence.map(|g| view! { <p class="word-gloss">{g}</p> })}
                        </li>
                    }).collect_view()}
                </ul>
            </div>
        }
    })
}

fn render_linked_words(
    linked_words: Vec<LinkedWord>,
    dialog_ref: NodeRef<html::Dialog>,
    lookup: ServerAction<GetWordDefinition>,
    current_word: RwSignal<String>,
    history: RwSignal<Vec<String>>,
) -> Option<impl IntoView> {
    (!linked_words.is_empty()).then(|| {
        view! {
            <div class="word-links">
                <h4>"Related words"</h4>
                <ul>
                    {linked_words.into_iter().map(|lw| {
                        let word_text = lw.target_text.clone();
                        view! {
                            <li>
                                <span
                                    class="clickable-word"
                                    on:click=move |_| {
                                        history.update(|h| h.push(current_word.get_untracked()));
                                        open_word_dialog(dialog_ref, lookup, current_word, word_text.clone());
                                    }
                                >
                                    {lw.target_text}
                                </span>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            </div>
        }
    })
}

fn render_lookup_result(
    lookup: ServerAction<GetWordDefinition>,
    current_word: RwSignal<String>,
    dialog_ref: NodeRef<html::Dialog>,
    history: RwSignal<Vec<String>>,
) -> impl IntoView {
    match lookup.value().get() {
        None => ().into_any(),
        Some(Err(e)) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
        Some(Ok(None)) => view! {
            <p>"No dictionary entry yet for " <strong>{current_word.get()}</strong> "."</p>
        }
        .into_any(),
        Some(Ok(Some(def))) => view! {
            <h3>{def.target_text}</h3>
            {def.part_of_speech.map(|p| view! { <p class="word-pos">{p}</p> })}
            {def.english_translation.map(|t| view! { <p class="word-gloss">{t}</p> })}
            {def.notes.map(|n| view! { <p>{n}</p> })}
            {render_examples(def.examples)}
            {render_linked_words(def.linked_words, dialog_ref, lookup, current_word, history)}
        }
        .into_any(),
    }
}

/// Renders `text` as a clickable word that opens a dismissable `<dialog>`
/// with its dictionary entry — part of speech, notes, an example sentence,
/// related words — or a graceful "no entry yet" state.
///
/// English glosses (an example's gloss sentence) only ever appear once
/// this dialog is explicitly opened by a click — never surfaced
/// automatically during review. This is purely an auxiliary,
/// user-triggered lookup layered on top of the existing four-button grade
/// row, never a change to it.
#[allow(clippy::must_use_candidate)]
#[component]
pub fn ClickableWord(text: String) -> impl IntoView {
    let dialog_ref: NodeRef<html::Dialog> = NodeRef::new();
    let lookup = ServerAction::<GetWordDefinition>::new();
    let current_word = RwSignal::new(text.clone());
    let history: RwSignal<Vec<String>> = RwSignal::new(Vec::new());

    let opener_text = text.clone();
    view! {
        <span class="clickable-word" on:click=move |_| {
            history.set(Vec::new());
            open_word_dialog(dialog_ref, lookup, current_word, opener_text.clone());
        }>
            {text}
        </span>
        <dialog node_ref=dialog_ref class="word-dialog" on:click=move |ev| on_backdrop_click(&ev, dialog_ref)>
            <button type="button" class="word-dialog-close" on:click=move |_| close_word_dialog(dialog_ref)>
                "Close"
            </button>
            {move || {
                (!history.get().is_empty()).then(|| {
                    view! {
                        <button
                            type="button"
                            on:click=move |_| {
                                if let Some(prev) = history.try_update(Vec::pop).flatten() {
                                    open_word_dialog(dialog_ref, lookup, current_word, prev);
                                }
                            }
                        >
                            "Back"
                        </button>
                    }
                })
            }}
            {move || render_lookup_result(lookup, current_word, dialog_ref, history)}
            {move || lookup.pending().get().then(|| view! { <p>"Looking up…"</p> })}
        </dialog>
    }
}

/// Optional metadata about a target-language word, captured alongside
/// card creation/editing — part of speech, notes, one example sentence,
/// related words — saved independently of any particular card (words are
/// keyed by their exact text, not by card id), via the same
/// `save_word_definition` write path a future popup-edit affordance would
/// also call.
#[allow(clippy::must_use_candidate)]
#[component]
pub fn DictionaryDetailsEditor(
    /// Tracks the *current* target_text input value reactively, so saving
    /// always writes against what's actually in the field right now, not
    /// a stale value from when the page loaded.
    target_text: Signal<String>,
    #[prop(default = String::new())] part_of_speech_initial: String,
    #[prop(default = String::new())] notes_initial: String,
    #[prop(default = String::new())] english_translation_initial: String,
    #[prop(default = String::new())] example_target_initial: String,
    #[prop(default = String::new())] example_gloss_initial: String,
    #[prop(default = String::new())] linked_words_initial: String,
) -> impl IntoView {
    let save = ServerAction::<SaveWordDefinition>::new();
    let part_of_speech: NodeRef<html::Input> = NodeRef::new();
    let notes: NodeRef<html::Textarea> = NodeRef::new();
    let english_translation: NodeRef<html::Input> = NodeRef::new();
    let example_target: NodeRef<html::Input> = NodeRef::new();
    let example_gloss: NodeRef<html::Input> = NodeRef::new();
    let linked_words: NodeRef<html::Input> = NodeRef::new();

    view! {
        <div class="card-section">
            <p class="card-section-heading">"Dictionary details (optional)"</p>
            <label>
                "English translation"
                <input type="text" node_ref=english_translation value=english_translation_initial />
            </label>
            <label>
                "Part of speech"
                <input type="text" node_ref=part_of_speech value=part_of_speech_initial />
            </label>
            <label>
                "Notes"
                <textarea node_ref=notes>{notes_initial}</textarea>
            </label>
            <label>
                "Example sentence (target language)"
                <input type="text" node_ref=example_target value=example_target_initial />
            </label>
            <label>
                "Example sentence gloss (English/source — shown only in the lookup popup, never during review)"
                <input type="text" node_ref=example_gloss value=example_gloss_initial />
            </label>
            <label>
                "Related words (comma separated)"
                <input type="text" node_ref=linked_words value=linked_words_initial />
            </label>
            <button
                type="button"
                on:click=move |_| {
                    save.dispatch(SaveWordDefinition {
                        target_text: target_text.get_untracked(),
                        part_of_speech: part_of_speech.get().map(|el| el.value()).unwrap_or_default(),
                        notes: notes.get().map(|el| el.value()).unwrap_or_default(),
                        english_translation: english_translation.get().map(|el| el.value()).unwrap_or_default(),
                        example_target_sentence: example_target.get().map(|el| el.value()).unwrap_or_default(),
                        example_gloss_sentence: example_gloss.get().map(|el| el.value()).unwrap_or_default(),
                        linked_words_raw: linked_words.get().map(|el| el.value()).unwrap_or_default(),
                    });
                }
            >
                "Save dictionary details"
            </button>
            {move || save.pending().get().then(|| view! { <p>"Saving…"</p> })}
            {move || {
                save.value().get().map(|r| match r {
                    Ok(()) => view! { <p>"Saved."</p> }.into_any(),
                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                })
            }}
        </div>
    }
}
