use leptos::html;
use leptos::prelude::*;

#[cfg(feature = "hydrate")]
use std::cell::RefCell;
#[cfg(feature = "hydrate")]
use std::rc::Rc;
#[cfg(feature = "hydrate")]
use web_sys::{MediaRecorder, MediaStream};

/// Past this many seconds of recording, a non-blocking warning is shown —
/// the user can keep going, this is a nudge, not a cutoff. Enforced client
/// side only.
pub const SOFT_DURATION_LIMIT_SECS: u32 = 30;

/// Hard ceiling on a single recording's size: recording stops itself
/// automatically the moment accumulated data crosses this, no further
/// input needed. This is a client-side convenience (stop before wasting
/// time on a doomed-to-be-rejected upload) — the real backstop is
/// `MAX_UPLOAD_BYTES` in main.rs's `DefaultBodyLimit`, set to the same
/// 50MB, which rejects an oversized request regardless of how it was
/// produced (recorded or picked via "Choose File").
pub const HARD_SIZE_LIMIT_BYTES: f64 = 50.0 * 1024.0 * 1024.0;

/// This component's reactive state, bundled into one `Copy` struct purely
/// to keep `browser::start_recording`'s argument list sane — it's exactly
/// the signals declared in `RecordButton` below, passed through as a unit
/// instead of five-plus separate parameters.
#[derive(Debug, Clone, Copy)]
struct RecorderSignals {
    recording: RwSignal<bool>,
    preview_url: RwSignal<Option<String>>,
    error: RwSignal<Option<String>>,
    elapsed_secs: RwSignal<u32>,
    stopped_notice: RwSignal<Option<String>>,
}

/// Records a short audio or video clip in-browser via MediaRecorder, then
/// attaches the result to `target_input` as though the user had picked it
/// via "Choose File" — so it rides through the existing multipart upload
/// path (`parse_card_multipart` in main.rs) completely unchanged.
///
/// The view tree below is identical in both the `ssr` and `hydrate`
/// builds (app.rs — and so this component — is shared between them); only
/// the click handler's actual behavior is feature-gated, in `browser`
/// below, since `web-sys`'s media APIs only exist in the `hydrate` build.
/// An earlier version of this component had two *separate* definitions
/// (a real one for hydrate, a `<button disabled>` stand-in for ssr) that
/// happened to render different DOM shapes — confirmed live in a browser
/// to crash Leptos hydration outright ("hydration error... expected an
/// HTML <button> element, but found #text", then an `unreachable` panic
/// that took the whole page down, not just this button). Keeping one
/// shared view tree is what actually prevents that class of bug, not
/// just a style preference.
// leptos's #[component] macro doesn't forward a #[must_use] on its input
// through to the function it generates, so clippy's must_use_candidate
// lint can't actually be satisfied here — silenced rather than ignored:
// discarding this call's return value would just mean the recorder never
// renders, so the lint's underlying concern is real, just unfixable in
// the way clippy expects for a #[component] fn.
#[allow(clippy::must_use_candidate)]
#[component]
pub fn RecordButton(
    /// "audio" or "video" — selects getUserMedia constraints and the
    /// preview element; the recorded clip is always webm (broadly
    /// supported by MediaRecorder without extra encoder setup).
    kind: &'static str,
    /// The file input this recording attaches itself to on stop.
    target_input: NodeRef<html::Input>,
) -> impl IntoView {
    let signals = RecorderSignals {
        recording: RwSignal::new(false),
        preview_url: RwSignal::new(None::<String>),
        error: RwSignal::new(None::<String>),
        elapsed_secs: RwSignal::new(0u32),
        // Set once the recording is stopped automatically (size limit)
        // rather than by the user clicking Stop — distinct from `error`,
        // since nothing actually went wrong here.
        stopped_notice: RwSignal::new(None::<String>),
    };
    let RecorderSignals {
        recording,
        preview_url,
        error,
        elapsed_secs,
        stopped_notice,
    } = signals;

    // Mutable handles that outlive any single render but aren't reactive
    // state themselves — only ever touched from closures running after a
    // user gesture, never during rendering. Only meaningful once
    // hydrated; declared (but never populated) in the ssr build purely so
    // the click handler below has something to pass by reference either
    // way, keeping its own shape identical across both builds too.
    #[cfg(feature = "hydrate")]
    let active_recorder: Rc<RefCell<Option<MediaRecorder>>> = Rc::new(RefCell::new(None));
    #[cfg(feature = "hydrate")]
    let active_stream: Rc<RefCell<Option<MediaStream>>> = Rc::new(RefCell::new(None));

    // The mic/camera indicator a browser shows stays lit for as long as a
    // MediaStream's tracks are live — not just while a recording is
    // actually in progress. Every normal stop path already releases them
    // (`on_stop_closure`/the error paths in `start_recording`, both via
    // `stop_tracks`), but confirmed live there was one gap: navigating
    // away mid-recording (a client-side route change, not a full page
    // reload) disposes this component without ever calling its own Stop
    // handler, and nothing was releasing the still-open stream when that
    // happened — the mic would stay hot indefinitely, with no recording
    // actually running and no UI left on screen to stop it from. on_cleanup
    // is Leptos's hook for exactly this: it runs on route-away disposal the
    // same as it would on a more ordinary unmount.
    #[cfg(feature = "hydrate")]
    {
        let active_recorder = active_recorder.clone();
        let active_stream = active_stream.clone();
        // on_cleanup requires Send + Sync even in this single-threaded wasm
        // build (see wiki_editor's autosave-loop cancellation flag for the
        // same constraint on a plain bool) — these are JS-backed types that
        // can't genuinely promise that, so SendWrapper stands in: its
        // unsafe Send/Sync impl only has to hold up because wasm32 never
        // actually crosses a thread boundary to begin with.
        let release = send_wrapper::SendWrapper::new(move || {
            browser::release_on_cleanup(&active_recorder, &active_stream);
        });
        on_cleanup(move || release.take()());
    }

    let on_toggle = move |_| {
        #[cfg(feature = "hydrate")]
        {
            if recording.get_untracked() {
                browser::stop_recording(&active_recorder);
            } else {
                browser::start_recording(
                    kind,
                    target_input,
                    signals,
                    active_recorder.clone(),
                    active_stream.clone(),
                );
            }
        }
        #[cfg(not(feature = "hydrate"))]
        {
            // Never actually reached — server-rendered event handlers
            // aren't invoked during SSR, and this becomes `browser`'s
            // real handler once hydrated. Referencing the props here
            // just keeps this arm's shape parallel to the one above.
            let _ = (kind, target_input, signals);
        }
    };

    view! {
        <span class="recorder">
            <button type="button" on:click=on_toggle>
                {move || if recording.get() { "Stop recording" } else { "Record" }}
            </button>
            {move || {
                recording
                    .get()
                    .then(|| {
                        let secs = elapsed_secs.get();
                        let over_soft_limit = secs >= SOFT_DURATION_LIMIT_SECS;
                        view! {
                            <span class="recorder-elapsed">{secs}"s"</span>
                            {over_soft_limit
                                .then(|| {
                                    view! {
                                        <span class="recorder-warning">
                                            "That's getting long — consider wrapping up."
                                        </span>
                                    }
                                })}
                        }
                    })
            }}
            {move || {
                stopped_notice
                    .get()
                    .map(|msg| view! { <span class="recorder-notice">{msg}</span> })
            }}
            {move || {
                error.get().map(|msg| view! { <span class="recorder-error">{msg}</span> })
            }}
            {move || {
                preview_url
                    .get()
                    .map(|url| {
                        if kind == "video" {
                            view! { <video controls src=url></video> }.into_any()
                        } else {
                            view! { <audio controls src=url></audio> }.into_any()
                        }
                    })
            }}
        </span>
    }
}

#[cfg(feature = "hydrate")]
mod browser {
    use super::HARD_SIZE_LIMIT_BYTES;
    use leptos::html;
    use leptos::prelude::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        Blob, BlobEvent, BlobPropertyBag, DataTransfer, File, FilePropertyBag, MediaRecorder,
        MediaRecorderOptions, MediaStream, MediaStreamConstraints, MediaStreamTrack, Url,
    };

    // dataavailable fires once per this many milliseconds while recording
    // (via MediaRecorder's own `timeslice` argument to `start`), which
    // doubles as a ~1-second tick for the elapsed-time/size checks below —
    // no separate timer needed.
    const TIMESLICE_MS: i32 = 1000;

    fn stop_tracks(active_stream: &Rc<RefCell<Option<MediaStream>>>) {
        if let Some(stream) = active_stream.borrow_mut().take() {
            for track in stream.get_tracks().iter() {
                if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                    track.stop();
                }
            }
        }
    }

    // Each `dataavailable` tick (see `TIMESLICE_MS`) accumulates the chunk
    // and the elapsed-time counter, then auto-stops once the hard size
    // limit is hit.
    fn on_data_available_closure(
        chunks: Rc<RefCell<Vec<Blob>>>,
        total_bytes: Rc<RefCell<f64>>,
        active_recorder: Rc<RefCell<Option<MediaRecorder>>>,
        elapsed_secs: RwSignal<u32>,
        stopped_notice: RwSignal<Option<String>>,
    ) -> Closure<dyn FnMut(BlobEvent)> {
        Closure::<dyn FnMut(BlobEvent)>::new(move |event: BlobEvent| {
            let Some(data) = event.data() else { return };
            if data.size() <= 0.0 {
                return;
            }
            *total_bytes.borrow_mut() += data.size();
            chunks.borrow_mut().push(data);
            elapsed_secs.set(elapsed_secs.get_untracked() + 1);

            if *total_bytes.borrow() >= HARD_SIZE_LIMIT_BYTES
                && let Some(media_recorder) = active_recorder.borrow().as_ref()
            {
                stopped_notice.set(Some(
                    "Stopped automatically — reached the 50MB size limit.".to_string(),
                ));
                let _ = media_recorder.stop();
            }
        })
    }

    // Assembles the accumulated chunks into the final Blob/File and
    // attaches it to `target_input`, exactly as though the user had
    // picked it via "Choose File".
    fn on_stop_closure(
        chunks: Rc<RefCell<Vec<Blob>>>,
        active_stream: Rc<RefCell<Option<MediaStream>>>,
        mime_type: String,
        target_input: NodeRef<html::Input>,
        recording: RwSignal<bool>,
        preview_url: RwSignal<Option<String>>,
    ) -> Closure<dyn FnMut()> {
        Closure::<dyn FnMut()>::new(move || {
            recording.set(false);

            let parts = js_sys::Array::new();
            for blob in chunks.borrow().iter() {
                parts.push(blob);
            }
            let blob_options = BlobPropertyBag::new();
            blob_options.set_type(&mime_type);
            let Ok(final_blob) = Blob::new_with_blob_sequence_and_options(&parts, &blob_options)
            else {
                stop_tracks(&active_stream);
                return;
            };

            if let Ok(url) = Url::create_object_url_with_blob(&final_blob) {
                preview_url.set(Some(url));
            }

            let file_options = FilePropertyBag::new();
            file_options.set_type(&mime_type);
            if let Ok(file) = File::new_with_blob_sequence_and_options(
                &js_sys::Array::of1(&final_blob),
                "recording.webm",
                &file_options,
            ) && let (Some(input), Ok(dt)) = (target_input.get_untracked(), DataTransfer::new())
            {
                let _ = dt.items().add_with_file(&file);
                input.set_files(dt.files().as_ref());
            }

            stop_tracks(&active_stream);
        })
    }

    // Called from on_cleanup, where the component's own signals
    // (recording/preview_url/...) may already be disposed — unlike
    // stop_recording below, this never routes through the MediaRecorder's
    // onstop handler (on_stop_closure writes to those signals and finishes
    // attaching the clip to target_input, neither of which makes sense, or
    // is even safe, once this component is gone). It clears the recorder's
    // own event handlers first so a stop() here can't trigger that handler
    // later anyway, then releases the hardware directly via stop_tracks —
    // the actual mic/camera-off step either path ultimately relies on.
    pub(super) fn release_on_cleanup(
        active_recorder: &Rc<RefCell<Option<MediaRecorder>>>,
        active_stream: &Rc<RefCell<Option<MediaStream>>>,
    ) {
        if let Some(media_recorder) = active_recorder.borrow_mut().take() {
            media_recorder.set_ondataavailable(None);
            media_recorder.set_onstop(None);
            let _ = media_recorder.stop();
        }
        stop_tracks(active_stream);
    }

    pub(super) fn stop_recording(active_recorder: &Rc<RefCell<Option<MediaRecorder>>>) {
        // Deliberately doesn't set `recording`/clear state itself — the
        // recorder's `onstop` handler (below) is the single place that
        // happens, so a size-triggered auto-stop and a manual Stop click
        // both leave the UI in the same, correct end state.
        if let Some(media_recorder) = active_recorder.borrow().as_ref() {
            let _ = media_recorder.stop();
        }
    }

    pub(super) fn start_recording(
        kind: &'static str,
        target_input: NodeRef<html::Input>,
        signals: super::RecorderSignals,
        active_recorder: Rc<RefCell<Option<MediaRecorder>>>,
        active_stream: Rc<RefCell<Option<MediaStream>>>,
    ) {
        let super::RecorderSignals {
            recording,
            preview_url,
            error,
            elapsed_secs,
            stopped_notice,
        } = signals;

        error.set(None);
        stopped_notice.set(None);
        elapsed_secs.set(0);
        // Revoke the previous preview, if any, rather than leaking object
        // URLs across repeated record attempts.
        if let Some(old_url) = preview_url.get_untracked() {
            let _ = Url::revoke_object_url(&old_url);
        }
        preview_url.set(None);

        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(media_devices) = window.navigator().media_devices() else {
            error.set(Some("This browser doesn't support recording.".to_string()));
            return;
        };

        let constraints = MediaStreamConstraints::new();
        constraints.set_audio(&JsValue::TRUE);
        if kind == "video" {
            constraints.set_video(&JsValue::TRUE);
        }
        let Ok(promise) = media_devices.get_user_media_with_constraints(&constraints) else {
            error.set(Some(
                "Couldn't request microphone/camera access.".to_string(),
            ));
            return;
        };

        let mime_type = if kind == "video" {
            "video/webm"
        } else {
            "audio/webm"
        };

        leptos::task::spawn_local(async move {
            let Ok(stream_js) = JsFuture::from(promise).await else {
                error.set(Some("Microphone/camera permission was denied.".to_string()));
                return;
            };
            let stream: MediaStream = stream_js.unchecked_into();
            active_stream.borrow_mut().replace(stream.clone());

            let options = MediaRecorderOptions::new();
            options.set_mime_type(mime_type);
            let Ok(media_recorder) =
                MediaRecorder::new_with_media_stream_and_media_recorder_options(&stream, &options)
            else {
                error.set(Some("Couldn't start the recorder.".to_string()));
                stop_tracks(&active_stream);
                return;
            };

            let chunks: Rc<RefCell<Vec<Blob>>> = Rc::new(RefCell::new(Vec::new()));
            let total_bytes: Rc<RefCell<f64>> = Rc::new(RefCell::new(0.0));

            // Safe to capture `active_recorder` now even though
            // `media_recorder` isn't stored in it until after `start()`
            // succeeds below: a dataavailable event can't fire until at
            // least one timeslice (1s) after that point, by which time the
            // cell is already populated.
            let on_data_available = on_data_available_closure(
                chunks.clone(),
                total_bytes.clone(),
                active_recorder.clone(),
                elapsed_secs,
                stopped_notice,
            );
            media_recorder.set_ondataavailable(Some(on_data_available.as_ref().unchecked_ref()));
            on_data_available.forget();

            let on_stop = on_stop_closure(
                chunks.clone(),
                active_stream.clone(),
                mime_type.to_string(),
                target_input,
                recording,
                preview_url,
            );
            media_recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));
            on_stop.forget();

            if media_recorder.start_with_time_slice(TIMESLICE_MS).is_err() {
                error.set(Some("Couldn't start the recorder.".to_string()));
                stop_tracks(&active_stream);
                return;
            }
            active_recorder.borrow_mut().replace(media_recorder);
            recording.set(true);
        });
    }
}
