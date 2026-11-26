//! Every change of any field whose path passes through a field named NAME, in any class:
//! tick, class, entity, path (indices), the field names along it, the value.
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

use anyhow::{Context as _, Result};
use haste::demofile::DemoFile;
use haste::entities::{DeltaHeader, Entity};
use haste::parser::{Context, Parser, Visitor};

struct Watch {
    name: String,
    last: HashMap<(i32, u64), String>,
}

impl Visitor for Watch {
    fn on_entity(&mut self, ctx: &Context, _h: DeltaHeader, entity: &Entity) -> Result<()> {
        for (key, value) in entity.iter() {
            let Some(path) = entity.get_path(key) else {
                continue;
            };
            let mut names = Vec::new();
            let mut field = path.get(0).and_then(|i| entity.serializer().get_child(i));
            names.push(field.map_or("?".into(), |f| f.var_name.str.to_string()));
            for i in path.iter().skip(1) {
                field = field.and_then(|f| f.get_child(*i as usize));
                names.push(field.map_or("?".into(), |f| f.var_name.str.to_string()));
            }
            if !names.iter().any(|n| n == &self.name) {
                continue;
            }
            let text = format!("{value:?}");
            if self
                .last
                .insert((entity.index(), *key), text.clone())
                .as_ref()
                != Some(&text)
            {
                let idx: Vec<String> = path.iter().map(|i| i.to_string()).collect();
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    ctx.tick(),
                    entity.serializer().serializer_name.str,
                    entity.index(),
                    idx.join("/"),
                    names.join("."),
                    text
                );
            }
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: field-watch <replay.dem> <field name>";
    let file = args.next().context(usage)?;
    let name = args.next().context(usage)?;
    let demo = DemoFile::start_reading(BufReader::new(File::open(file)?))?;
    let mut parser = Parser::from_stream_with_visitor(
        demo,
        Watch {
            name,
            last: HashMap::new(),
        },
    )?;
    parser.run_to_end()
}
