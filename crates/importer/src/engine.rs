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

/// Ollama defaulted to a 4096-token context window regardless of what the
/// model itself supported, which overflowed on a single dense dictionary
/// photo — this mirrors the override env var the original script used
/// (`OLLAMA_NUM_CTX`) under its new name.
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

/// `gpt-oss` (the decompose model) uses `OpenAI`'s "harmony" response
/// format: it always reasons in an `analysis` channel before answering in
/// a `final` channel, and — confirmed in a real run against the full-size
/// production model — that reasoning can run for several thousand tokens.
/// This literal text is what marks the start of the real answer.
const GPT_OSS_FINAL_CHANNEL_TRIGGER: &str = "<|channel|>final<|message|>";

fn penalty_sampler(model: &LlamaModel) -> LlamaSampler {
    // Without this, greedy decoding has no defense against repetition
    // loops — confirmed in a real run against the production decompose
    // model: an open-ended schema field (part_of_speech) degenerated into
    // hundreds of repeated "?"/"." characters once the model had nothing
    // confident left to say. The original Ollama-based prototype never
    // hit this because Ollama applies its own default repeat penalty
    // (repeat_last_n=64, repeat_penalty=1.1) unconditionally; this chain,
    // built from scratch, had never replicated it.
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
/// tokens into `ctx`. See the module doc comment for why this never calls
/// `LlamaSampler::sample()`.
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
        let ctx_params = LlamaContextParams::default().with_n_ctx(Some(self.n_ctx));
        let mut ctx = self
            .model
            .new_context(&self.backend, ctx_params)
            .map_err(|e| format!("failed to create context: {e}"))?;

        // Without this, an instruct-tuned model sees a raw completion
        // prompt rather than its own chat format, isn't in "answer this"
        // mode, and under grammar-constrained greedy decoding takes the
        // path of least resistance — closing a JSON array immediately as
        // `[]`. Confirmed this was silently happening (misdiagnosed at the
        // time as a tiny-model capability limit) until a real end-to-end
        // run against the full-size production model reproduced the exact
        // same empty-array output.
        let chat_template = self
            .model
            .chat_template(None)
            .map_err(|e| format!("chat_template failed: {e:?}"))?;
        let msg = LlamaChatMessage::new("user".to_string(), prompt.to_string())
            .map_err(|e| e.to_string())?;
        let formatted = self
            .model
            .apply_chat_template(&chat_template, &[msg], true)
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
        if let Some(schema) = schema {
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
        } else {
            let mut sampler = build_sampler(&self.model, None)?;
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
        let ctx_params = LlamaContextParams::default().with_n_ctx(Some(self.n_ctx));
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
        let mut full_prompt = prompt.to_string();
        full_prompt.push_str(mtmd_default_marker());
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

        let n_past = chunks
            .eval_chunks(&mut self.mtmd_ctx, &mut ctx, 0, 0, 1, true)
            .map_err(|e| format!("mtmd eval_chunks failed: {e:?}"))?;

        let n_ctx_i32 = i32::try_from(self.n_ctx.get()).map_err(|e| e.to_string())?;
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
