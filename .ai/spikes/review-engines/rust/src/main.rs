use automerge::{AutoCommit, Cursor, LoadOptions, ReadDoc, TextEncoding, ROOT, transaction::Transactable};
use yrs::{Doc, GetString, OffsetKind, Options, ReadTxn, StateVector, StickyIndex, Text, Transact, Update};
use yrs::updates::decoder::Decode;
use serde_json::{Value, json};
use std::{error::Error, fs, path::PathBuf};

fn bytes(value: &Value) -> Vec<u8> {
    value.as_array().unwrap().iter().map(|n| n.as_u64().unwrap() as u8).collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let fixtures=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures");
    let anchors: Value=serde_json::from_slice(&fs::read(fixtures.join("anchors.json"))?)?;
    let expected_start=anchors["start"].as_u64().unwrap() as usize;
    let expected_end=anchors["end"].as_u64().unwrap() as usize;
    let mut am=AutoCommit::load_with_options(&fs::read(fixtures.join("automerge-js.bin"))?,
        LoadOptions::new().text_encoding(TextEncoding::Utf16CodeUnit))?;
    let (_,text_object)=am.get(ROOT,"text")?.unwrap();
    let start=Cursor::try_from(anchors["automerge"]["start"].as_str().unwrap())?;
    let end=Cursor::try_from(anchors["automerge"]["end"].as_str().unwrap())?;
    let am_start=am.get_cursor_position(&text_object,&start,None)?;
    let am_end=am.get_cursor_position(&text_object,&end,None)?;
    assert_eq!((am_start,am_end),(expected_start,expected_end));
    assert_eq!(am.text(&text_object)?,anchors["text"].as_str().unwrap());
    am.splice_text(&text_object,am_start+3,0,"!")?;
    fs::write(fixtures.join("automerge-rust.bin"),am.save())?;
    let am_text=am.text(&text_object)?;

    let yd=Doc::with_options(Options {offset_kind:OffsetKind::Utf16,..Options::default()});
    let yt=yd.get_or_insert_text("text");
    yd.transact_mut().apply_update(Update::decode_v1(&fs::read(fixtures.join("yjs-js.bin"))?)?)?;
    let ys=StickyIndex::decode_v1(&bytes(&anchors["yjs"]["start"]))?;
    let ye=StickyIndex::decode_v1(&bytes(&anchors["yjs"]["end"]))?;
    let (y_start,y_end)={let txn=yd.transact();
        (ys.get_offset(&txn).unwrap().index,ye.get_offset(&txn).unwrap().index)};
    assert_eq!((y_start as usize,y_end as usize),(expected_start,expected_end));
    assert_eq!(yt.get_string(&yd.transact()),anchors["text"].as_str().unwrap());
    yt.insert(&mut yd.transact_mut(),y_start+3,"!");
    fs::write(fixtures.join("yjs-rust.bin"),yd.transact().encode_state_as_update_v1(&StateVector::default()))?;
    let result=json!({"automerge":{"start":am_start,"end":am_end,"text":am_text},
        "yrs":{"start":y_start,"end":y_end,"text":yt.get_string(&yd.transact())},
        "encoding":"Explicit UTF-16 in both native libraries"});
    fs::write(fixtures.join("rust-results.json"),serde_json::to_vec_pretty(&result)?)?;
    println!("{}",result);
    Ok(())
}
