// Benchmark-only renderer. Compile against the existing local backend Cargo dependencies.
#[allow(dead_code)]
#[path = "../../../backend/inference/src/generation/policy.rs"]
mod policy;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let prompt: serde_json::Value = serde_json::from_reader(std::io::stdin())?;
    let slots = prompt["passages"].as_array().ok_or("missing passages")?.len();
    let user = serde_json::to_string(&prompt)?.replace('<', "\\u003c").replace('>', "\\u003e");
    let result = serde_json::json!({
        "model": "qwen3-4b-instruct-2507-q4-k-m-3605803b-v1", "temperature":0,
        "max_tokens":640, "stream":false,
        "messages":[{"role":"system","content":policy::answer_prompt(false)},
                    {"role":"user","content":user}],
        "response_format":{"type":"json_schema","schema":policy::quote_schema(false, slots)}
    });
    serde_json::to_writer(std::io::stdout(), &result)?;
    Ok(())
}
