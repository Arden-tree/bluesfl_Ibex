use regex::Regex;
use rig::completion::Usage;
use serde_json::Value;
use std::collections::HashMap;

pub fn parse_json_md(ctx: &str) -> anyhow::Result<Value> {
    let re = Regex::new(r"```json\s*(.*[\s\S]*?)\s*```").unwrap();
    let re_raw = Regex::new(r"\s*(\{.*[\s\S]*\})\s*").unwrap();

    if let Some(captures) = re.captures(ctx).or(re_raw.captures(ctx)) {
        let json_str = captures.get(1).unwrap().as_str();
        let parsed_json: Value = serde_json::from_str(json_str)?;

        Ok(parsed_json)
    } else {
        // No valid JSON found
        anyhow::bail!("No valid JSON block found")
    }
}

pub fn token_price(token_usage: &Usage, model: &str) -> Option<f64> {
    let pricing: HashMap<&'static str, (f64, f64)> = HashMap::from([
        // --- GPT-4.1 and GPT-4o series ---
        ("gpt-4.1", (2.00, 8.00)),
        ("gpt-4.1-mini", (0.40, 1.60)),
        ("gpt-4.1-nano", (0.10, 0.40)),
        ("gpt-4o", (2.50, 10.00)),
        ("gpt-4o-2024-05-13", (5.00, 15.00)),
        ("gpt-4o-mini", (0.15, 0.60)),
        // --- GPT-4 legacy/turbo ---
        ("chatgpt-4o-latest", (5.00, 15.00)),
        ("gpt-4-turbo-2024-04-09", (10.00, 30.00)),
        ("gpt-4-0125-preview", (10.00, 30.00)),
        ("gpt-4-1106-preview", (10.00, 30.00)),
        ("gpt-4-0613", (30.00, 60.00)),
        ("gpt-4-0314", (30.00, 60.00)),
        ("gpt-4-32k", (60.00, 120.00)),
        // --- GPT-3.5 series ---
        ("gpt-3.5-turbo", (0.50, 1.50)),
        ("gpt-3.5-turbo-0125", (0.50, 1.50)),
        ("gpt-3.5-turbo-1106", (1.00, 2.00)),
        ("gpt-3.5-turbo-0613", (1.50, 2.00)),
        ("gpt-3.5-0301", (1.50, 2.00)),
        ("gpt-3.5-turbo-instruct", (1.50, 2.00)),
        ("gpt-3.5-turbo-16k-0613", (3.00, 4.00)),
        // --- Other GPT / O-series ---
        ("gpt-realtime", (4.00, 16.00)),
        ("gpt-audio", (2.50, 10.00)),
        ("o1", (15.00, 60.00)),
        ("o1-pro", (150.00, 600.00)),
        ("o3-pro", (20.00, 80.00)),
        ("o3", (2.00, 8.00)),
        ("o3-deep-research", (10.00, 40.00)),
        ("o4-mini", (1.10, 4.40)),
        ("o4-mini-deep-research", (2.00, 8.00)),
        ("o3-mini", (1.10, 4.40)),
        ("o1-mini", (1.10, 4.40)),
        // Claude
        ("claude-haiku-4-5", (1.0, 5.0)),
        ("claude-haiku-4-5-20251001", (1.0, 5.0)),
        ("claude-sonnet-4-5", (3.0, 15.0)),
        ("claude-sonnet-4-5-20250929", (3.0, 15.0)),
        ("claude-opus-4-1-20250805", (15.0, 75.0)),
        ("claude-opus-4-0", (15.0, 75.0)),
        ("claude-opus-4-20250514", (15.0, 75.0)),
        ("claude-sonnet-4-0", (3.0, 15.0)),
        ("claude-sonnet-4-20250514", (3.0, 15.0)),
        ("claude-3-7-sonnet-20250219", (3.0, 15.0)),
        ("claude-3-7-sonnet-latest", (3.0, 15.0)),
        ("claude-3-5-haiku-20241022", (1.0, 5.0)),
        ("claude-3-5-haiku-latest", (1.0, 5.0)),
        ("claude-3-5-sonnet-20241022", (3.0, 15.0)),
        ("claude-3-5-sonnet-latest", (3.0, 15.0)),
        ("claude-3-5-sonnet-20240620", (3.0, 15.0)),
        ("claude-3-haiku-20240307", (0.25, 1.25)),
        ("claude-3-opus-20240229", (15.0, 75.0)),
        ("claude-3-opus-latest", (15.0, 75.0)),
        ("claude-3-sonnet-20240229", (3.0, 15.0)),
    ]);

    pricing
        .get(model.trim())
        .map(|(input_per_million, output_per_million)| {
            let input_cost = *input_per_million * (token_usage.input_tokens as f64) / 1_000_000.0;
            let output_cost =
                *output_per_million * (token_usage.output_tokens as f64) / 1_000_000.0;
            input_cost + output_cost
        })
}
