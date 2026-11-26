//! For every signed-integer field (decoded as zigzag varint): how its values move.
//! A field written as a plain varint but read as zigzag shows raw steps of 1 where the
//! decoded value jumps across zero (11, 10, 9 read as -6, 5, -5).
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

use anyhow::{Context as _, Result};
use haste::demofile::DemoFile;
use haste::entities::{DeltaHeader, Entity};
use haste::fieldvalue::FieldValue;
use haste::parser::{Context, Parser, Visitor};

#[derive(Default)]
struct Stat {
    changes: u64,
    negatives: u64,
    step1_decoded: u64,
    step1_raw: u64,
    min: i64,
    max: i64,
    encoder: String,
}

#[derive(Default)]
struct Survey {
    last: HashMap<(i32, u64), i64>,
    stats: HashMap<(String, String, String), Stat>,
}

fn raw(v: i64) -> i64 {
    if v >= 0 { 2 * v } else { -2 * v - 1 }
}

impl Visitor for Survey {
    fn on_entity(&mut self, _ctx: &Context, _h: DeltaHeader, entity: &Entity) -> Result<()> {
        for (key, value) in entity.iter() {
            let FieldValue::I64(v) = value else { continue };
            let prev = self.last.insert((entity.index(), *key), *v);
            if prev == Some(*v) {
                continue;
            }
            let Some(path) = entity.get_path(key) else {
                continue;
            };
            let Some(field) = entity.get_serializer_field(path) else {
                continue;
            };
            let k = (
                entity.serializer().serializer_name.str.to_string(),
                field.var_name.str.to_string(),
                field.var_type.str.to_string(),
            );
            let s = self.stats.entry(k).or_insert_with(|| Stat {
                min: i64::MAX,
                max: i64::MIN,
                encoder: field
                    .var_encoder
                    .as_ref()
                    .map(|e| e.str.to_string())
                    .unwrap_or_default(),
                ..Default::default()
            });
            s.changes += 1;
            s.negatives += (*v < 0) as u64;
            s.min = s.min.min(*v);
            s.max = s.max.max(*v);
            if let Some(p) = prev {
                s.step1_decoded += ((v - p).abs() == 1) as u64;
                s.step1_raw += ((raw(*v) - raw(p)).abs() == 1) as u64;
            }
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: int-survey <replay.dem>")?;
    let demo = DemoFile::start_reading(BufReader::new(File::open(path)?))?;
    let mut parser = Parser::from_stream_with_visitor(demo, Survey::default())?;
    parser.run_to_end()?;
    println!("class\tfield\ttype\tencoder\tchanges\tnegatives\tstep1_decoded\tstep1_raw\tmin\tmax");
    let mut rows: Vec<_> = parser.visitor().stats.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    for ((c, f, t), s) in rows {
        println!(
            "{c}\t{f}\t{t}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            s.encoder, s.changes, s.negatives, s.step1_decoded, s.step1_raw, s.min, s.max
        );
    }
    Ok(())
}
