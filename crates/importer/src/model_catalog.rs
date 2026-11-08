//! The default models this tool downloads and caches on first run.
//! Pinned exactly (repo, filename, sha256) rather than resolved
//! dynamically — `HuggingFace` repos can add/rename files over time, and a
//! silent switch to a different quantization is exactly the kind of thing
//! that should require a deliberate version bump of this file, not happen
//! automatically. All three confirmed present on their respective
//! `ggml-org` repos (the same org llama.cpp's own maintainers publish
//! under) at the time this was written.
pub struct ModelSpec {
    pub repo: &'static str,
    pub file: &'static str,
    pub sha256: &'static str,
}

pub const SEGMENT_MODEL: ModelSpec = ModelSpec {
    repo: "ggml-org/Qwen2.5-VL-7B-Instruct-GGUF",
    file: "Qwen2.5-VL-7B-Instruct-Q4_K_M.gguf",
    sha256: "9258bf05b12686d097ff3b6b18d968ab393649780aa2b3cd67fec43d50554392",
};

pub const SEGMENT_MMPROJ: ModelSpec = ModelSpec {
    repo: "ggml-org/Qwen2.5-VL-7B-Instruct-GGUF",
    file: "mmproj-Qwen2.5-VL-7B-Instruct-Q8_0.gguf",
    sha256: "2ddb555391bae966e412deab9e07b58afa18bcc06930ba0f1c78a3695ab9e506",
};

pub const DECOMPOSE_MODEL: ModelSpec = ModelSpec {
    repo: "ggml-org/gpt-oss-20b-GGUF",
    file: "gpt-oss-20b-MXFP4.gguf",
    sha256: "27cd6c432c7672cb812a92f611cf3ba7bbc35928262bb1e1253ff4ee6ae35901",
};
