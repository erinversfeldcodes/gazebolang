//! Wraps the embedded local-LLM inference engine (`llama-cpp-2`, a Rust
//! binding to llama.cpp) — this is what replaces the separately-running
//! Ollama server the original script talked to over HTTP.
//!
//! Grammar-constrained (JSON-schema) sampling has an open upstream crash
//! report, `utilityai/llama-cpp-rs#1007`
//! (`GGML_ASSERT(!stacks.empty())` on the first `LlamaSampler::sample()`
//! call) — confirmed reproducing on this exact dependency version during
//! this project's own de-risking spike, including combined with `mtmd`
//! image input. Worked around throughout this module by never calling
//! `sample()`: instead, logits are pulled manually
//! (`LlamaContext::get_logits_ith`) and the sampler chain is applied to a
//! `LlamaTokenDataArray` directly — confirmed crash-free on both the
//! text-only and vision+grammar paths in that same spike.
//!
//! That workaround isn't sufficient for `gpt-oss` (the decompose model):
//! it needs its grammar enforced *lazily*, only once its "final channel"
//! marker appears (see `GPT_OSS_FINAL_CHANNEL_TRIGGER`), and `llama-cpp-2`'s
//! own lazy-grammar sampler (`LlamaSampler::grammar_lazy_patterns`) hits
//! this exact same upstream assertion the moment its trigger fires,
//! confirmed in a real run against the production model — a different
//! code path than the one the manual-sampling workaround above avoids.
//! `generate_with_lazy_grammar` hand-rolls the same behavior instead: no
//! grammar sampler exists at all until the trigger text is matched as a
//! plain substring of the output so far, at which point a fresh, eager
//! grammar sampler (the already-proven-safe kind) is constructed and used
//! for the remainder of generation.
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaModel};
use llama_cpp_2::mtmd::{
    MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText, mtmd_default_marker,
};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::token::data::LlamaTokenData;
use llama_cpp_2::token::data_array::LlamaTokenDataArray;
use serde_json::Value;
use std::ffi::CString;
use std::num::NonZeroU32;
use std::path::Path;

fn num_ctx() -> NonZeroU32 {
    let n = std::env::var("IMPORTER_NUM_CTX")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(16384);
    NonZeroU32::new(n).unwrap_or(NonZeroU32::new(16384).unwrap())
}

fn n_threads() -> i32 {
    std::thread::available_parallelism().map_or(4, |n| i32::try_from(n.get()).unwrap_or(4))
}

/// `n_batch` (the max tokens a single prefill `decode()` call may submit
/// at once) defaults to a value smaller than `n_ctx` and is a *separate*
/// setting from it — confirmed by a real crash
/// (`GGML_ASSERT(n_tokens_all <= cparams.n_batch)`) once a prompt long
/// enough to exceed that default (two dense dictionary entries, for the
/// boundary-continuation check) was decoded in one prefill. Matching it to
/// `n_ctx` removes this as a limit anywhere a long prompt gets prefilled
/// in one shot.
///
/// `n_ubatch` (the physical micro-batch size the KV cache allocator
/// actually uses for `memory->init_batch`, per llama.cpp's own
/// `llama-context.cpp`) is yet another separate setting, defaulting far
/// smaller still — confirmed by a real run: even after fixing `n_batch`
/// and a real `eval_chunks` call correctly submitting a large image as
/// one batch, generation still failed with "failed to find a memory slot"
/// immediately afterward, every time, at the same token count regardless
/// of how large `n_ctx` was set.
///
/// Deliberately NOT matched to `n_ctx` the way `n_batch` is: setting it
/// that large (confirmed in a real run) makes llama.cpp pre-allocate a
/// Metal compute buffer sized for an `n_ubatch`-token batch *regardless
/// of how many tokens a given call actually uses* — at `n_ctx` = 16384
/// that was ~10.9GB, which genuinely ran the GPU out of memory, and even
/// 8192 still needed ~5.4GB (confirmed identical whether the real image
/// batch was 3996 or 4050 tokens — the buffer size tracks `n_ubatch`
/// itself, not actual content size). `n_ubatch` only needs to cover the
/// single largest batch this module ever actually submits in one
/// `decode()` call, which is a `VisionEngine` image prefill — real runs
/// this session needed 3996-4050 tokens for both a single page and a
/// two-page composite (this model's image-token count doesn't scale
/// with input resolution the way raw pixel area might suggest) — not
/// tied to `n_ctx` at all, and sized with real but modest headroom over
/// that, not an excessive multiple of it.
const N_UBATCH: u32 = 4608;

fn ctx_params_for(n_ctx: NonZeroU32) -> LlamaContextParams {
    LlamaContextParams::default()
        .with_n_ctx(Some(n_ctx))
        .with_n_batch(n_ctx.get())
        .with_n_ubatch(N_UBATCH)
}

/// `gpt-oss` (the decompose model) uses `OpenAI`'s "harmony" response
/// format: it always reasons in an `analysis` channel before answering in
/// a `final` channel, and — confirmed in a real run against the full-size
/// production model — that reasoning can run for several thousand tokens.
/// This literal text is what marks the start of the real answer.
const GPT_OSS_FINAL_CHANNEL_TRIGGER: &str = "<|channel|>final<|message|>";

/// This task is a few bounded fields pulled from one short excerpt —
/// it never needs the model's deepest reasoning tier, and a real run
/// against the production model showed the cost of not pinning this
/// explicitly: with no reasoning effort set at all (see
/// `gpt_oss_system_message`'s own doc comment for why it's otherwise
/// unset here), it sometimes reasoned for thousands of tokens and
/// never reached `GPT_OSS_FINAL_CHANNEL_TRIGGER` before running out of
/// context. "low" targets that failure directly, rather than just
/// giving it more context to maybe still run out of.
const GPT_OSS_REASONING_EFFORT: &str = "low";

/// Reconstructs, by hand, the exact system message `gpt-oss`'s own
/// GGUF `chat_template` would build from its `reasoning_effort` kwarg —
/// confirmed by reading that template's "System Message Construction"
/// macro directly (identity line, knowledge cutoff, current date, a
/// "Reasoning: <effort>" line, then the channel-validity note). This
/// step is necessary only because of a real gap in the binding this
/// project uses: `apply_chat_template` calls `llama_chat_apply_template`,
/// which renders `gpt-oss`'s Harmony format via a hardcoded C++
/// shortcut (matched by template name, not by executing the model's
/// actual Jinja template) that passes a system message's content
/// through completely verbatim and has no concept of `reasoning_effort`
/// at all — confirmed by reading that C++ implementation directly.
/// Passing no system message, as this project did until a real run
/// surfaced the stuck-reasoning failure above, silently leaves the
/// model defaulting to "medium" effort with none of the rest of this
/// text either, which is a stranger, less-trained-on input shape than
/// either a real system message or none at all.
fn gpt_oss_system_message(reasoning_effort: &str) -> String {
    let today = time::OffsetDateTime::now_utc().date();
    format!(
        "You are ChatGPT, a large language model trained by OpenAI.\n\
         Knowledge cutoff: 2024-06\n\
         Current date: {:04}-{:02}-{:02}\n\n\
         Reasoning: {reasoning_effort}\n\n\
         # Valid channels: analysis, commentary, final. Channel must be included for every message.",
        today.year(),
        u8::from(today.month()),
        today.day()
    )
}

fn penalty_sampler(model: &LlamaModel) -> LlamaSampler {
    // Without this, greedy decoding has no defense against repetition
    // loops
    LlamaSampler::penalties(model.n_vocab(), 64, 1.1, 0.0, 0.0)
}

/// Builds a grammar-constrained sampler chain from a JSON Schema, applied
/// from the very first generated token, or a plain greedy
/// (temperature-0-equivalent) chain when no schema is given — same
/// determinism rationale the original script's `temperature: 0` had:
/// small models are measurably more prone to structural/content errors at
/// higher temperatures on this kind of multi-field extraction.
///
/// Only correct for a model with no reasoning-channel behavior of its
/// own (the segment/vision model, or no schema at all) — see
/// `generate_with_lazy_grammar` for `gpt-oss`.
fn build_sampler(model: &LlamaModel, schema: Option<&Value>) -> Result<LlamaSampler, String> {
    match schema {
        Some(schema) => {
            let schema_str = serde_json::to_string(schema).map_err(|e| e.to_string())?;
            let grammar_str = llama_cpp_2::json_schema_to_grammar(&schema_str)
                .map_err(|e| format!("json_schema_to_grammar failed: {e}"))?;
            let grammar_sampler = LlamaSampler::grammar(model, &grammar_str, "root")
                .map_err(|e| format!("LlamaSampler::grammar failed: {e}"))?;
            Ok(LlamaSampler::chain_simple([
                grammar_sampler,
                penalty_sampler(model),
                LlamaSampler::greedy(),
            ]))
        }
        None => Ok(LlamaSampler::chain_simple([
            penalty_sampler(model),
            LlamaSampler::greedy(),
        ])),
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The shared generation loop, used by both `TextEngine` and
/// `VisionEngine` once their respective prefill has put `start_n_past`
/// tokens into `ctx`.
fn generate_with_workaround(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    batch: &mut LlamaBatch,
    sampler: &mut LlamaSampler,
    start_n_past: i32,
    n_ctx: i32,
) -> Result<String, String> {
    let n_vocab = model.n_vocab();
    let mut n_cur = start_n_past;
    let mut output_bytes = Vec::new();

    while n_cur < n_ctx {
        let logits = ctx.get_logits_ith(-1);
        let candidates: Vec<LlamaTokenData> = logits
            .iter()
            .enumerate()
            .map(|(id, &logit)| {
                LlamaTokenData::new(LlamaToken(i32::try_from(id).unwrap_or(n_vocab)), logit, 0.0)
            })
            .collect();
        let mut data_array = LlamaTokenDataArray::new(candidates, false);
        data_array.apply_sampler(sampler);
        let token = data_array
            .selected_token()
            .ok_or("sampler chain did not select a token")?;
        sampler.accept(token);

        if model.vocab().is_eog(token) {
            break;
        }
        output_bytes.extend_from_slice(&model.vocab().token_to_piece(token, true, None));

        batch.clear();
        batch
            .add(token, n_cur, &[0], true)
            .map_err(|e| e.to_string())?;
        n_cur += 1;
        ctx.decode(batch)
            .map_err(|e| format!("llama_decode failed during generation: {e}"))?;
    }

    Ok(String::from_utf8_lossy(&output_bytes).into_owned())
}

/// Lets the model reason completely unconstrained (just penalty + greedy,
/// no grammar sampler exists yet) until `trigger` first appears as a
/// plain substring of the output so far, then constructs a fresh eager
/// grammar sampler and switches to it for the rest of generation. See the
/// module doc comment for why this hand-rolls what `grammar_lazy_patterns`
/// is supposed to do. Returns only the text generated after the trigger
/// — the model's reasoning preamble is discarded, not returned to the
/// caller.
fn generate_with_lazy_grammar(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    batch: &mut LlamaBatch,
    start_n_past: i32,
    n_ctx: i32,
    grammar_str: &str,
    trigger: &str,
) -> Result<String, String> {
    let n_vocab = model.n_vocab();
    let mut n_cur = start_n_past;
    let mut output_bytes: Vec<u8> = Vec::new();
    let mut sampler = LlamaSampler::chain_simple([penalty_sampler(model), LlamaSampler::greedy()]);
    let mut answer_start: Option<usize> = None;

    while n_cur < n_ctx {
        let logits = ctx.get_logits_ith(-1);
        let candidates: Vec<LlamaTokenData> = logits
            .iter()
            .enumerate()
            .map(|(id, &logit)| {
                LlamaTokenData::new(LlamaToken(i32::try_from(id).unwrap_or(n_vocab)), logit, 0.0)
            })
            .collect();
        let mut data_array = LlamaTokenDataArray::new(candidates, false);
        data_array.apply_sampler(&mut sampler);
        let token = data_array
            .selected_token()
            .ok_or("sampler chain did not select a token")?;
        sampler.accept(token);

        if model.vocab().is_eog(token) {
            break;
        }
        output_bytes.extend_from_slice(&model.vocab().token_to_piece(token, true, None));

        if answer_start.is_none()
            && let Some(pos) = find_subslice(&output_bytes, trigger.as_bytes())
        {
            answer_start = Some(pos + trigger.len());
            let grammar_sampler = LlamaSampler::grammar(model, grammar_str, "root")
                .map_err(|e| format!("LlamaSampler::grammar failed: {e}"))?;
            sampler = LlamaSampler::chain_simple([
                grammar_sampler,
                penalty_sampler(model),
                LlamaSampler::greedy(),
            ]);
        }

        batch.clear();
        batch
            .add(token, n_cur, &[0], true)
            .map_err(|e| e.to_string())?;
        n_cur += 1;
        ctx.decode(batch)
            .map_err(|e| format!("llama_decode failed during generation: {e}"))?;
    }

    match answer_start {
        Some(start) => Ok(String::from_utf8_lossy(&output_bytes[start..]).into_owned()),
        None => Err(format!(
            "model never reached its \"{trigger}\" marker within the context window — it got stuck reasoning"
        )),
    }
}

/// Tokenizes `prompt` via the model's own chat template (no image) and
/// runs the eager (schema-from-token-1) generation loop. Used by
/// `TextEngine::generate`'s no-schema path — not used for `gpt-oss`'s
/// schema-constrained path, which needs the lazy-trigger workaround in
/// `generate_with_lazy_grammar` instead. (A pure-text, no-image path on
/// `VisionEngine` was tried too, for a boundary-continuation judgment in
/// `segment.rs` — confirmed unreliable for that model in a real run, so
/// that call now goes through `VisionEngine::generate`'s image+text path
/// instead, and this helper is `TextEngine`-only again.)
fn generate_text_eager(
    model: &LlamaModel,
    backend: &LlamaBackend,
    n_ctx: NonZeroU32,
    prompt: &str,
    schema: Option<&Value>,
) -> Result<String, String> {
    let ctx_params = ctx_params_for(n_ctx);
    let mut ctx = model
        .new_context(backend, ctx_params)
        .map_err(|e| format!("failed to create context: {e}"))?;

    let chat_template = model
        .chat_template(None)
        .map_err(|e| format!("chat_template failed: {e:?}"))?;
    let system_msg = LlamaChatMessage::new(
        "system".to_string(),
        gpt_oss_system_message(GPT_OSS_REASONING_EFFORT),
    )
    .map_err(|e| e.to_string())?;
    let msg =
        LlamaChatMessage::new("user".to_string(), prompt.to_string()).map_err(|e| e.to_string())?;
    let formatted = model
        .apply_chat_template(&chat_template, &[system_msg, msg], true)
        .map_err(|e| format!("apply_chat_template failed: {e:?}"))?;

    let n_ctx_i32 = i32::try_from(n_ctx.get()).map_err(|e| e.to_string())?;
    let tokens = model.vocab().tokenize(formatted.as_bytes(), true, true);
    let token_count =
        i32::try_from(tokens.len()).map_err(|_| "prompt is absurdly long".to_string())?;
    if token_count >= n_ctx_i32 {
        return Err(format!(
            "prompt has {token_count} tokens, which doesn't fit in the {n_ctx}-token context (set IMPORTER_NUM_CTX higher)"
        ));
    }
    let mut batch = LlamaBatch::new(n_ctx.get() as usize, 1);
    let last_index = token_count - 1;
    for (i, token) in (0_i32..).zip(tokens) {
        batch
            .add(token, i, &[0], i == last_index)
            .map_err(|e| e.to_string())?;
    }
    ctx.decode(&mut batch)
        .map_err(|e| format!("llama_decode failed on the prompt: {e}"))?;

    let mut sampler = build_sampler(model, schema)?;
    let n_past = batch.n_tokens();
    generate_with_workaround(model, &mut ctx, &mut batch, &mut sampler, n_past, n_ctx_i32)
}

/// A loaded text model, reused across many `generate()` calls within one
/// pipeline stage — loading a multi-GB GGUF file is far too slow to repeat
/// per call, so `decompose.rs` loads this once and calls `generate()` once
/// per enumerate/fill request.
pub struct TextEngine {
    backend: LlamaBackend,
    model: LlamaModel,
    n_ctx: NonZeroU32,
}

impl TextEngine {
    pub fn load(model_path: &Path) -> Result<Self, String> {
        let backend = LlamaBackend::init().map_err(|e| e.to_string())?;
        let model_params = LlamaModelParams::default().with_n_gpu_layers(1000);
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|e| format!("failed to load model {}: {e}", model_path.display()))?;
        Ok(Self {
            backend,
            model,
            n_ctx: num_ctx(),
        })
    }

    pub fn generate(&self, prompt: &str, schema: Option<&Value>) -> Result<String, String> {
        // Without the chat template, an instruct-tuned model sees a raw
        // completion prompt rather than its own chat format, isn't in
        // "answer this" mode, and under grammar-constrained greedy
        // decoding takes the path of least resistance — closing a JSON
        // array immediately as `[]`.
        let Some(schema) = schema else {
            return generate_text_eager(&self.model, &self.backend, self.n_ctx, prompt, None);
        };

        let ctx_params = ctx_params_for(self.n_ctx);
        let mut ctx = self
            .model
            .new_context(&self.backend, ctx_params)
            .map_err(|e| format!("failed to create context: {e}"))?;

        let chat_template = self
            .model
            .chat_template(None)
            .map_err(|e| format!("chat_template failed: {e:?}"))?;
        let system_msg = LlamaChatMessage::new(
            "system".to_string(),
            gpt_oss_system_message(GPT_OSS_REASONING_EFFORT),
        )
        .map_err(|e| e.to_string())?;
        let msg = LlamaChatMessage::new("user".to_string(), prompt.to_string())
            .map_err(|e| e.to_string())?;
        let formatted = self
            .model
            .apply_chat_template(&chat_template, &[system_msg, msg], true)
            .map_err(|e| format!("apply_chat_template failed: {e:?}"))?;

        let n_ctx_i32 = i32::try_from(self.n_ctx.get()).map_err(|e| e.to_string())?;
        let tokens = self
            .model
            .vocab()
            .tokenize(formatted.as_bytes(), true, true);
        let token_count =
            i32::try_from(tokens.len()).map_err(|_| "prompt is absurdly long".to_string())?;
        if token_count >= n_ctx_i32 {
            return Err(format!(
                "prompt has {token_count} tokens, which doesn't fit in the {}-token context (set IMPORTER_NUM_CTX higher)",
                self.n_ctx
            ));
        }
        let mut batch = LlamaBatch::new(self.n_ctx.get() as usize, 1);
        let last_index = token_count - 1;
        for (i, token) in (0_i32..).zip(tokens) {
            batch
                .add(token, i, &[0], i == last_index)
                .map_err(|e| e.to_string())?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| format!("llama_decode failed on the prompt: {e}"))?;

        let n_past = batch.n_tokens();
        let schema_str = serde_json::to_string(schema).map_err(|e| e.to_string())?;
        let grammar_str = llama_cpp_2::json_schema_to_grammar(&schema_str)
            .map_err(|e| format!("json_schema_to_grammar failed: {e}"))?;
        generate_with_lazy_grammar(
            &self.model,
            &mut ctx,
            &mut batch,
            n_past,
            n_ctx_i32,
            &grammar_str,
            GPT_OSS_FINAL_CHANNEL_TRIGGER,
        )
    }
}

/// A loaded vision model + multimodal projector, reused across many
/// `generate()` calls — same rationale as `TextEngine`.
pub struct VisionEngine {
    backend: LlamaBackend,
    model: LlamaModel,
    mtmd_ctx: MtmdContext,
    n_ctx: NonZeroU32,
}

impl VisionEngine {
    pub fn load(model_path: &Path, mmproj_path: &Path) -> Result<Self, String> {
        let backend = LlamaBackend::init().map_err(|e| e.to_string())?;
        let model_params = LlamaModelParams::default().with_n_gpu_layers(1000);
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|e| format!("failed to load model {}: {e}", model_path.display()))?;

        let mmproj_str = mmproj_path
            .to_str()
            .ok_or_else(|| format!("mmproj path isn't valid UTF-8: {}", mmproj_path.display()))?;
        let mtmd_params = MtmdContextParams {
            use_gpu: true,
            print_timings: false,
            n_threads: n_threads(),
            media_marker: CString::new(mtmd_default_marker()).map_err(|e| e.to_string())?,
            image_min_tokens: -1,
            image_max_tokens: -1,
        };
        let mtmd_ctx = MtmdContext::init_from_file(mmproj_str, &model, &mtmd_params)
            .map_err(|e| format!("failed to load mmproj {}: {e:?}", mmproj_path.display()))?;

        Ok(Self {
            backend,
            model,
            mtmd_ctx,
            n_ctx: num_ctx(),
        })
    }

    pub fn generate(
        &mut self,
        prompt: &str,
        image_path: &Path,
        schema: Option<&Value>,
    ) -> Result<String, String> {
        let ctx_params = ctx_params_for(self.n_ctx);
        let mut ctx = self
            .model
            .new_context(&self.backend, ctx_params)
            .map_err(|e| format!("failed to create context: {e}"))?;

        let image_str = image_path
            .to_str()
            .ok_or_else(|| format!("image path isn't valid UTF-8: {}", image_path.display()))?;
        let bitmap = MtmdBitmap::from_file(&self.mtmd_ctx, image_str, false)
            .map_err(|e| format!("failed to load image {}: {e:?}", image_path.display()))?;

        let chat_template = self
            .model
            .chat_template(None)
            .map_err(|e| format!("chat_template failed: {e:?}"))?;
        let mut full_prompt = mtmd_default_marker().to_string();
        full_prompt.push_str(prompt);
        let msg =
            LlamaChatMessage::new("user".to_string(), full_prompt).map_err(|e| e.to_string())?;
        let formatted = self
            .model
            .apply_chat_template(&chat_template, &[msg], true)
            .map_err(|e| format!("apply_chat_template failed: {e:?}"))?;

        let input_text = MtmdInputText {
            text: formatted,
            add_special: true,
            parse_special: true,
        };
        let chunks = self
            .mtmd_ctx
            .tokenize(input_text, &[&bitmap])
            .map_err(|e| format!("mtmd tokenize failed: {e:?}"))?;

        let n_ctx_i32 = i32::try_from(self.n_ctx.get()).map_err(|e| e.to_string())?;
        let n_past = chunks
            .eval_chunks(&mut self.mtmd_ctx, &mut ctx, 0, 0, n_ctx_i32, true)
            .map_err(|e| format!("mtmd eval_chunks failed: {e:?}"))?;
        let mut sampler = build_sampler(&self.model, schema)?;
        let mut batch = LlamaBatch::new(self.n_ctx.get() as usize, 1);
        generate_with_workaround(
            &self.model,
            &mut ctx,
            &mut batch,
            &mut sampler,
            n_past,
            n_ctx_i32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt_oss_system_message_sets_the_requested_reasoning_effort() {
        let msg = gpt_oss_system_message("low");
        assert!(msg.contains("Reasoning: low"));
    }

    #[test]
    fn gpt_oss_system_message_keeps_the_channel_validity_note() {
        let msg = gpt_oss_system_message("low");
        assert!(msg.contains("# Valid channels: analysis, commentary, final."));
    }
}
