//! Bounded, opt-in views. Budgets include the Brainkeep envelope, not JSON-RPC transport.
use serde_json::{Value, json};

fn size(v: &Value) -> usize {
    json!({"trust":"data_only","data":v}).to_string().len()
}
fn shortened(value: &str, budget: usize) -> String {
    let mut out = String::new();
    for c in value.chars() {
        out.push(c);
        if json!(out).to_string().len() > budget {
            out.pop();
            break;
        }
    }
    out
}
fn preview(mut item: Value) -> Value {
    let mut fields = Vec::new();
    for (key, budget) in [
        ("title", 100),
        ("content", 150),
        ("source", 100),
        ("agent", 50),
        ("topic", 80),
    ] {
        if let Some(full) = item[key].as_str() {
            let brief = shortened(full, budget);
            if brief != full {
                fields.push(key);
                item[key] = json!(brief);
            }
        }
    }
    if item["truncated"] == true && !fields.contains(&"content") {
        fields.push("content");
    }
    let obj = item.as_object_mut().expect("store row");
    // Project is selected by the request; repeating it in every result wastes context.
    obj.remove("project");
    obj.remove("source_verified");
    obj.remove("truncated");
    item["truncated_fields"] = json!(fields);
    item
}

pub fn render(name: &str, value: Value, args: &Value) -> Result<Value, String> {
    match name {
        "remember" | "correct" => Ok(
            json!({"id":value["id"],"revision":value["revision"],"state":value["state"],"saved_at":value["saved_at"],"source_verified":false}),
        ),
        "search" | "context" | "history" => {
            let history = name == "history";
            let budget = match name {
                "search" => 2048,
                "context" => 4096,
                _ => 16384,
            };
            let input = value["items"].as_array().ok_or("invalid memory list")?;
            let offset = args["offset"].as_u64().unwrap_or(0);
            let more = value["more_available"] == true || !value["next_after_revision"].is_null();
            let mut output = json!({"items":[],"expand_with":"recall","source_verified":false});
            if !history {
                output["snapshot"] = value["snapshot"].clone();
            }
            for (i, item) in input.iter().enumerate() {
                let mut trial = output.clone();
                trial["items"]
                    .as_array_mut()
                    .unwrap()
                    .push(preview(item.clone()));
                if history {
                    trial["next_after_revision"] = json!(item["revision"]);
                } else {
                    trial["next_offset"] = json!(offset + i as u64 + 1);
                }
                if size(&trial) > budget {
                    break;
                }
                output = trial;
            }
            let count = output["items"].as_array().unwrap().len();
            if count == 0 && !input.is_empty() {
                return Err("preview exceeds budget; recall by id".into());
            }
            let next_key = if history {
                "next_after_revision"
            } else {
                "next_offset"
            };
            if count == input.len() && !more {
                output[next_key] = Value::Null;
            }
            // Store rejects continuations when a write changed the snapshot generation.
            Ok(output)
        }
        "recall" => {
            let offset = match args.get("offset") {
                None => 0,
                Some(n) => n.as_u64().ok_or("invalid offset")? as usize,
            };
            let mut output = value.clone();
            let full = value["content"].as_str().ok_or("invalid memory content")?;
            let boundaries: Vec<usize> = full
                .char_indices()
                .map(|(n, _)| n)
                .chain(std::iter::once(full.len()))
                .collect();
            let length = boundaries.len() - 1;
            if offset > length {
                return Err("offset exceeds content length".into());
            }
            let mut lo = offset;
            let mut hi = length;
            output["offset"] = json!(offset);
            // Reserve the cursor before fitting. UTF-8 stays whole; JSON escaping counts.
            while lo < hi {
                let mid = lo + (hi - lo).div_ceil(2);
                output["content"] = json!(&full[boundaries[offset]..boundaries[mid]]);
                output["next_offset"] = if mid < length {
                    json!(mid)
                } else {
                    Value::Null
                };
                if size(&output) <= 16384 {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            output["content"] = json!(&full[boundaries[offset]..boundaries[lo]]);
            output["next_offset"] = if lo < length { json!(lo) } else { Value::Null };
            if size(&output) > 16384 || (lo == offset && lo < length) {
                return Err("metadata exceeds detail budget".into());
            }
            Ok(output)
        }
        _ => Ok(value),
    }
}
