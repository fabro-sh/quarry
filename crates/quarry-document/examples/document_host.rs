//! JSON pipe used to verify the browser's requests against the native server
//! implementation. No JavaScript reconstruction participates in the round trip.
use quarry_document::{CommandRequest, Document, SeedBlock};
use serde::Deserialize;
use std::io::{self, Read};

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    bytes: Option<Vec<u8>>,
    #[serde(default)]
    blocks: Vec<SeedBlock>,
    #[serde(default)]
    requests: Vec<CommandRequest>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let input: Input = serde_json::from_str(&input)?;
    let mut document = match input.bytes {
        Some(bytes) => Document::load(&bytes)?,
        None => Document::with_id("interop-fixture")?,
    };
    for block in input.blocks {
        document.insert_block(block)?;
    }
    for request in input.requests {
        document.apply_request(&request)?;
    }
    println!(
        "{}",
        serde_json::json!({"bytes":document.save(),"view":document.view()?})
    );
    Ok(())
}
