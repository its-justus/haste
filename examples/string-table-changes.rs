//! Checks `Visitor::on_string_table` against the old way of following a string table.
//!
//! The old way (the prototype's replay reader): after every string-table message, compare
//! every entry of the table with the data last seen and keep those that differ. The new
//! way: look only at the entries the message touched. Both must give the same changes, in
//! the same order. Prints the number of changes and whether the two agree.
//!
//!   string-table-changes <replay.dem> [table, default ActiveModifiers]
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

use anyhow::{Context as _, Result, ensure};
use haste::demofile::DemoFile;
use haste::parser::{Context, Parser, Visitor};
use haste::stringtables::StringTable;

type Change = (i32, i32, Vec<u8>); // tick, entry, data

fn data(table: &StringTable, index: i32) -> Vec<u8> {
    table
        .get_item(&index)
        .and_then(|item| item.user_data.as_ref())
        // SAFETY: the parser is not running while the visitor looks at its tables.
        .map(|d| unsafe { (*d.get()).clone() })
        .unwrap_or_default()
}

struct Compare {
    name: String,
    seen_all: HashMap<i32, Vec<u8>>,
    seen_touched: HashMap<i32, Vec<u8>>,
    by_diff: Vec<Change>,
    by_touched: Vec<Change>,
}

impl Visitor for Compare {
    fn on_string_table(
        &mut self,
        ctx: &Context,
        table: &StringTable,
        changed: &[i32],
    ) -> Result<()> {
        if table.name() != self.name {
            return Ok(());
        }
        // old: the whole table
        let mut all: Vec<i32> = table.items().map(|(i, _)| *i).collect();
        all.sort();
        for i in all {
            let d = data(table, i);
            if self.seen_all.get(&i) != Some(&d) {
                self.seen_all.insert(i, d.clone());
                self.by_diff.push((ctx.tick(), i, d));
            }
        }
        // new: only what the message touched
        let mut touched = changed.to_vec();
        touched.sort();
        touched.dedup();
        for i in touched {
            let d = data(table, i);
            if self.seen_touched.get(&i) != Some(&d) {
                self.seen_touched.insert(i, d.clone());
                self.by_touched.push((ctx.tick(), i, d));
            }
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let file = args
        .next()
        .context("usage: string-table-changes <replay.dem> [table]")?;
    let name = args.next().unwrap_or_else(|| "ActiveModifiers".to_string());
    let demo = DemoFile::start_reading(BufReader::new(File::open(file)?))?;
    let visitor = Compare {
        name,
        seen_all: HashMap::new(),
        seen_touched: HashMap::new(),
        by_diff: Vec::new(),
        by_touched: Vec::new(),
    };
    let mut parser = Parser::from_stream_with_visitor(demo, visitor)?;
    parser.run_to_end()?;
    let v = parser.visitor();
    println!(
        "changes: whole-table diff {}, touched entries {}",
        v.by_diff.len(),
        v.by_touched.len()
    );
    ensure!(v.by_diff == v.by_touched, "the two ways differ");
    println!("identical");
    Ok(())
}
